"""Retained and calibration-run outputs for qualification production."""

from __future__ import annotations

import json

from .contract_primitives import (
    assert_retained_json_privacy,
    canonical_sha256,
    require_positive_int,
    write_private_json,
)
from .foundation import LOWER_TIER_NONCLAIMS, require
from .qualification_metrics import QualificationMeasurementEvidence
from .qualification_production_types import (
    QualificationProducerContext,
    QualificationRunnerEvidence,
    QualificationScenarioEvidence,
)


def retained_qualification_output(
    context: QualificationProducerContext,
    runner: QualificationRunnerEvidence,
    scenarios: QualificationScenarioEvidence,
    measurements: QualificationMeasurementEvidence,
) -> dict:
    identity = context.runtime["identity"]
    retained = {
        "schema_version": 1,
        "status": runner.expected_status,
        "tier": context.args.proof_tier,
        "source": context.manifest["source"],
        "package": {
            **context.package,
            **context.contracts,
            "matrix_cell_id": runner.matrix_cell_id,
            "accelerator_claim": runner.matrix_cell["accelerator_claim"],
            "model_sha256": identity["embedding_model_sha256"],
            "backend": identity["embedding_backend"],
            "policy": identity["embedding_policy"],
            "cache_state": measurements.host["cache_state"],
            "residency_state": measurements.host["residency_state"],
        },
        "host": measurements.host,
        "same_account": context.runtime["same_account"],
        "shared_identity": scenarios.shared_identity,
        "timing": measurements.timing,
        "scenarios": scenarios.scenarios,
        "lower_tier_nonclaims": {
            claim: {
                "claimed": False,
                "reason": (
                    "this exact-package qualification tier does not establish "
                    "the broader claim"
                ),
            }
            for claim in sorted(LOWER_TIER_NONCLAIMS)
        },
        "metrics": measurements.metrics,
    }
    if context.args.proof_tier == "installed_runtime":
        retained["installed_plugin"] = context.runtime["installed_plugin"]
        retained["managed_runtime"] = context.runtime["managed_runtime"]
    return retained


def _calibration_memory_samples(memory: dict) -> list[dict]:
    samples = []
    for sample in memory["payload"]["samples"]:
        normalized = json.loads(json.dumps(sample))
        normalized["process"] = normalized.pop("producer_process")
        samples.append(normalized)
    return samples


def calibration_run_output(
    context: QualificationProducerContext,
    runner: QualificationRunnerEvidence,
    measurements: QualificationMeasurementEvidence,
) -> dict:
    run_index = require_positive_int(
        context.args.calibration_run_index,
        "--calibration-run-index",
    )
    require(
        run_index <= 3,
        "--calibration-run-index must be in the preregistered range 1..3",
    )
    identity = context.runtime["identity"]
    package = {
        "archive_sha256": context.archive_sha256,
        "executable_sha256": context.manifest["binary"]["sha256"],
        "asset_target": context.manifest["asset_target"],
        "release_version": context.manifest["release_version"],
        "model_sha256": identity["embedding_model_sha256"],
        "policy": context.args.engine_policy,
        "backend": runner.expected_backend,
    }
    contracts = {
        "protocol_sha256": context.measurement_contract["protocol_sha256"],
        "measurement_protocol_sha256": context.measurement_contract[
            "measurement_protocol_sha256"
        ],
        "input_constant_set_sha256": context.measurement_contract[
            "constant_set_sha256"
        ],
    }
    metrics = json.loads(json.dumps(measurements.measurement["payload"]["metrics"]))
    metrics["total_codestory_process_memory"] = {
        "unit": "bytes",
        "samples": _calibration_memory_samples(measurements.memory),
    }
    identity_seed = {
        "source": context.manifest["source"],
        "package": package,
        "matrix_cell_id": runner.matrix_cell_id,
        "run_index": run_index,
        "host_fingerprint": measurements.host["fingerprint"],
        "measurement_artifact_sha256": measurements.measurement["artifact"]["sha256"],
        "memory_artifact_sha256": measurements.memory["artifact"]["sha256"],
    }
    run_id = canonical_sha256(identity_seed)
    raw_payload = {
        "schema_version": 1,
        "run_id_sha256": run_id,
        "matrix_cell_id": runner.matrix_cell_id,
        "run_index": run_index,
        "host_fingerprint": measurements.host["fingerprint"],
        "source": context.manifest["source"],
        "contracts": contracts,
        "package": package,
        "clean": context.manifest["source"]["tracked_dirty"] is False,
        "unplanned_suspend": measurements.measurement["unplanned_suspend"],
        "metrics": metrics,
    }
    return {
        "run_id_sha256": run_id,
        "matrix_cell_id": runner.matrix_cell_id,
        "run_index": run_index,
        "host_fingerprint": measurements.host["fingerprint"],
        "clean": raw_payload["clean"],
        "unplanned_suspend": raw_payload["unplanned_suspend"],
        "source": context.manifest["source"],
        "contracts": contracts,
        "package": package,
        "raw_artifact": {
            "name": "measurements.raw.json",
            "sha256": canonical_sha256(raw_payload),
            "payload": raw_payload,
        },
    }


def write_qualification_outputs(
    context: QualificationProducerContext,
    runner: QualificationRunnerEvidence,
    scenarios: QualificationScenarioEvidence,
    measurements: QualificationMeasurementEvidence,
) -> dict:
    retained = retained_qualification_output(
        context,
        runner,
        scenarios,
        measurements,
    )
    write_private_json(context.args.qualification_evidence, retained)
    assert_retained_json_privacy(
        context.args.qualification_evidence,
        [
            *context.forbidden_values,
            *context.runtime.get("_qualification_forbidden_values", []),
        ],
    )
    if (
        context.args.proof_tier == "calibration"
        and context.args.calibration_run_output is not None
    ):
        write_private_json(
            context.args.calibration_run_output,
            calibration_run_output(context, runner, measurements),
        )
    return retained
