"""Retained outputs for frozen-candidate qualification production."""

from __future__ import annotations

from .contract_primitives import (
    assert_retained_json_privacy,
    write_private_json,
)
from .foundation import LOWER_TIER_NONCLAIMS
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
    return retained
