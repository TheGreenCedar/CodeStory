"""Qualification shared-identity and scenario artifact collection."""

from __future__ import annotations

from .contract_primitives import (
    require_exact_keys,
    require_nonempty_string,
    require_positive_int,
)
from .foundation import REQUIRED_SERVER_SCENARIOS, require
from .qualification_artifacts import qualification_artifact
from .qualification_production_types import (
    QualificationExternalEvidence,
    QualificationProducerContext,
    QualificationRunnerEvidence,
    QualificationScenarioEvidence,
)


def _qualification_shared_identity(context: QualificationProducerContext) -> dict:
    shared = context.runtime["shared_identity"]
    require_exact_keys(
        shared,
        {
            "endpoint_namespace_id",
            "lifetime_authority_id",
            "listener_id",
            "server_instance_id",
            "server_process_start_id",
            "engine_owner_id",
            "native_worker_id",
            "load_generation",
            "model_load_count",
        },
        "live two-host shared identity",
    )
    for field in (
        "endpoint_namespace_id",
        "lifetime_authority_id",
        "listener_id",
        "server_instance_id",
        "server_process_start_id",
        "engine_owner_id",
        "native_worker_id",
    ):
        require_nonempty_string(shared[field], f"live shared_identity.{field}")
    require_positive_int(
        shared["load_generation"],
        "live shared_identity.load_generation",
    )
    require(
        shared["model_load_count"] == 1,
        "qualification runner cold race did not preserve one model load",
    )
    return shared


def _qualification_scenario(
    scenario_id: str,
    *,
    context: QualificationProducerContext,
    runner: QualificationRunnerEvidence,
    external: QualificationExternalEvidence,
) -> dict:
    required_assertions = set(
        context.measurement_contract["measurement_protocol"]["scenario_contracts"][
            scenario_id
        ]["required"]
    )
    artifact, assertions = qualification_artifact(
        context.artifact_root,
        runner.output["scenarios"][scenario_id],
        scenario_id=scenario_id,
        contracts=context.contracts,
        package=context.package,
        same_account=context.runtime["same_account"],
        materialization=context.runtime["materialization"],
        nonce_sha256=context.nonce_sha256,
        forbidden_values=context.forbidden_values,
    )
    external_artifacts = []
    if scenario_id in {"server_crash", "worker_stall"}:
        for assertion, value in external.publication_fault["assertions"].items():
            require(
                assertion in required_assertions,
                f"publication fault evidence derived unknown assertion {assertion}",
            )
            assertions[assertion] = value
        external_artifacts.append(external.publication_fault["artifact"])
        if (
            scenario_id == "server_crash"
            and external.fault_recovery_consistency is not None
        ):
            external_artifacts.append(external.fault_recovery_consistency["artifact"])
    require(
        set(assertions) == required_assertions,
        f"qualification scenario {scenario_id} derived assertion set differs from its preregistered contract",
    )
    require(
        all(value is True for value in assertions.values()),
        f"qualification scenario {scenario_id} has a failed assertion",
    )
    return {
        "status": "pass",
        "assertions": assertions,
        "artifacts": [artifact, *external_artifacts],
    }


def collect_qualification_scenarios(
    context: QualificationProducerContext,
    runner: QualificationRunnerEvidence,
    external: QualificationExternalEvidence,
) -> QualificationScenarioEvidence:
    raw_scenarios = runner.output["scenarios"]
    require(
        isinstance(raw_scenarios, dict)
        and set(raw_scenarios) == REQUIRED_SERVER_SCENARIOS,
        "qualification runner returned an incomplete scenario set",
    )
    scenarios = {
        scenario_id: _qualification_scenario(
            scenario_id,
            context=context,
            runner=runner,
            external=external,
        )
        for scenario_id in sorted(REQUIRED_SERVER_SCENARIOS)
    }
    return QualificationScenarioEvidence(
        _qualification_shared_identity(context),
        scenarios,
    )
