"""Self-tests for qualification phase contracts."""

from __future__ import annotations

import copy
from types import SimpleNamespace
from unittest.mock import patch, sentinel

from . import qualification_metrics, qualification_workflow
from .foundation import ProofFailure, require
from .qualification_metrics import (
    _qualification_cache_state_from_scenarios,
    _qualification_host,
)
from .qualification_production_types import (
    QualificationRunnerEvidence,
    QualificationScenarioEvidence,
)
from .qualification_scenario_evidence import validate_replay_attempts, validate_retry_state


def _replay_attempt(
    ordinal: int,
    request_id: str,
    server_instance_id: str,
    outcome: str,
    loss_code: str | None = None,
) -> dict[str, object]:
    attempt: dict[str, object] = {
        "ordinal": ordinal,
        "request_id": request_id,
        "server_instance_id": server_instance_id,
        "submitted_ns": ordinal * 10,
        "completed_ns": ordinal * 10 + 1,
        "outcome": outcome,
    }
    if loss_code is not None:
        attempt["loss_code"] = loss_code
    return attempt


def _qualification_cache_state_self_tests() -> None:
    context = SimpleNamespace(
        args=SimpleNamespace(engine_policy="accelerated"),
        runtime={
            "identity": {
                "embedding_backend": "metal",
                "embedding_engine_residency": "resident",
            },
            "same_account": {"account_id": "uid:501"},
            "materialization": {
                "sha256": "a" * 64,
                "reused_on_rejoin": False,
            },
        },
        manifest={"asset_target": "macos-arm64"},
    )
    runner = QualificationRunnerEvidence(
        output={},
        expected_status="pass",
        expected_backend="metal",
        matrix_cell_id="protected_macos_arm64_metal",
        matrix_cell={
            "host_class": "protected_self_hosted_macos_arm64",
            "accelerator_claim": "metal",
            "cache_state": "reused",
        },
    )
    reused_scenario = QualificationScenarioEvidence(
        shared_identity={},
        scenarios={
            "true_idle_respawn": {
                "assertions": {"verified_materialization_reused": True}
            }
        },
    )
    measurement = {"unplanned_suspend": False}
    cache_state = _qualification_cache_state_from_scenarios(
        runner,
        reused_scenario,
    )
    host = _qualification_host(
        context,
        runner,
        measurement,
        cache_state=cache_state,
    )
    require(
        context.runtime["materialization"]["reused_on_rejoin"] is False
        and host["cache_state"] == "reused",
        "cold bootstrap state overrode proved qualification replacement reuse",
    )

    for hostile, message in (
        (
            QualificationScenarioEvidence(
                shared_identity={},
                scenarios={
                    "true_idle_respawn": {
                        "assertions": {"verified_materialization_reused": False}
                    }
                },
            ),
            "false qualification replacement reuse was accepted",
        ),
        (
            QualificationScenarioEvidence(
                shared_identity={},
                scenarios={"true_idle_respawn": {"assertions": {}}},
            ),
            "missing qualification replacement reuse proof was accepted",
        ),
    ):
        try:
            _qualification_cache_state_from_scenarios(
                runner,
                hostile,
            )
        except ProofFailure:
            pass
        else:
            raise ProofFailure(message)


def _qualification_measurement_dataflow_self_test() -> None:
    measurement_context = SimpleNamespace(
        measurement_contract={
            "constant_set": {"status": "frozen"},
            "measurement_protocol": {"required_metrics": ["sentinel_metric"]},
        },
        contracts={"constant_set_sha256": "a" * 64},
    )
    runner = SimpleNamespace(matrix_cell={"cache_state": "reused"})
    scenarios = QualificationScenarioEvidence(
        shared_identity={},
        scenarios={
            "true_idle_respawn": {
                "assertions": {"verified_materialization_reused": True}
            }
        },
    )
    retained_measurement = {"unplanned_suspend": False}
    real_cache_state = qualification_metrics._qualification_cache_state_from_scenarios

    def derive_cache_state(actual_runner, actual_scenarios):
        require(
            actual_runner is runner and actual_scenarios is scenarios,
            "qualification measurement collection replaced validated runner or scenarios",
        )
        return real_cache_state(actual_runner, actual_scenarios)

    def retain_metric(metric, *, context, measurement, memory):
        require(
            metric == "sentinel_metric"
            and context is measurement_context
            and measurement is retained_measurement
            and memory is sentinel.memory,
            "qualification measurement collection replaced metric phase evidence",
        )
        return sentinel.metric

    def retain_host(actual_context, actual_runner, actual_measurement, *, cache_state):
        require(
            actual_context is measurement_context
            and actual_runner is runner
            and actual_measurement is retained_measurement
            and cache_state == "reused",
            "qualification host did not receive the proved cache state",
        )
        return sentinel.host

    with (
        patch.object(
            qualification_metrics,
            "_qualification_measurement_sources",
            return_value=(retained_measurement, sentinel.memory),
        ),
        patch.object(
            qualification_metrics,
            "_qualification_cache_state_from_scenarios",
            side_effect=derive_cache_state,
        ),
        patch.object(
            qualification_metrics,
            "_retained_qualification_metric",
            side_effect=retain_metric,
        ),
        patch.object(
            qualification_metrics,
            "_qualification_host",
            side_effect=retain_host,
        ),
    ):
        retained = qualification_metrics.collect_qualification_measurements(
            measurement_context,
            runner,
            scenarios,
        )
    require(
        retained.measurement is retained_measurement
        and retained.memory is sentinel.memory
        and retained.host is sentinel.host
        and retained.metrics == {"sentinel_metric": sentinel.metric},
        "qualification measurement collection returned stale phase evidence",
    )


def _qualification_workflow_dataflow_self_test() -> None:
    validated_scenarios = QualificationScenarioEvidence(
        shared_identity={"server_instance_id": "validated"},
        scenarios={"true_idle_respawn": {"assertions": {}}},
    )

    def retain_measurements(context, runner, scenarios):
        require(
            context is sentinel.context
            and runner is sentinel.runner
            and scenarios is validated_scenarios,
            "qualification workflow replaced validated scenarios before measurement retention",
        )
        return sentinel.measurements

    def retain_outputs(context, runner, scenarios, measurements):
        require(
            context is sentinel.context
            and runner is sentinel.runner
            and scenarios is validated_scenarios
            and measurements is sentinel.measurements,
            "qualification workflow replaced validated producer phase evidence",
        )
        return sentinel.output

    with (
        patch.object(
            qualification_workflow,
            "prepare_qualification_producer",
            return_value=sentinel.context,
        ),
        patch.object(
            qualification_workflow,
            "collect_qualification_external_evidence",
            return_value=sentinel.external,
        ),
        patch.object(
            qualification_workflow,
            "run_qualification_producer",
            return_value=sentinel.runner,
        ),
        patch.object(
            qualification_workflow,
            "collect_qualification_scenarios",
            return_value=validated_scenarios,
        ),
        patch.object(
            qualification_workflow,
            "collect_qualification_measurements",
            side_effect=retain_measurements,
        ),
        patch.object(
            qualification_workflow,
            "write_qualification_outputs",
            side_effect=retain_outputs,
        ),
    ):
        result = qualification_workflow.produce_qualification_evidence(
            sentinel.args,
            sentinel.qualification_cli,
            sentinel.env,
            sentinel.root,
            sentinel.runtime,
            sentinel.manifest,
            sentinel.archive_sha256,
            sentinel.measurement_contract,
            sentinel.server_cleanup_control,
        )
    require(result is sentinel.output, "qualification workflow returned stale evidence")


def run_qualification_self_tests() -> None:
    _qualification_cache_state_self_tests()
    _qualification_measurement_dataflow_self_test()
    _qualification_workflow_dataflow_self_test()
    retry = validate_retry_state(
        {
            "code": "embedding_server_owner_unresponsive",
            "message_head": "owner is frozen",
            "retry_class": "after_server_change",
            "retry_after_ms": 0,
            "retry_condition": "server identity changes",
        },
        "self-test retry",
    )
    require(
        retry.code == "embedding_server_owner_unresponsive"
        and retry.retry_class == "after_server_change",
        "typed retry validation changed",
    )
    invalid_retry = {
        "code": "embedding_server_owner_unresponsive",
        "message_head": "owner is frozen",
        "retry_class": "invented",
        "retry_after_ms": 0,
        "retry_condition": "server identity changes",
    }
    try:
        validate_retry_state(invalid_retry, "self-test invalid retry")
    except ProofFailure:
        pass
    else:
        raise ProofFailure("unknown retry class was accepted")

    replay = {
        "wire_attempt_count": 2,
        "wire_attempts": [
            _replay_attempt(
                1,
                "request-1",
                "server-old",
                "server_loss",
                loss_code="embedding_server_connection_lost",
            ),
            _replay_attempt(2, "request-2", "server-new", "completed"),
        ],
    }
    attempts = validate_replay_attempts(
        replay,
        old_server_instance_id="server-old",
        new_server_instance_id="server-new",
    )
    require(
        attempts[0].outcome == "server_loss"
        and attempts[0].loss_code == "embedding_server_connection_lost"
        and attempts[1].outcome == "completed"
        and attempts[1].loss_code is None,
        "typed replay validation changed",
    )
    stale_replay = copy.deepcopy(replay)
    stale_replay["wire_attempts"][1]["server_instance_id"] = "server-old"
    try:
        validate_replay_attempts(
            stale_replay,
            old_server_instance_id="server-old",
            new_server_instance_id="server-new",
        )
    except ProofFailure:
        pass
    else:
        raise ProofFailure("replay against the stale server was accepted")
    misclassified_replay = copy.deepcopy(replay)
    misclassified_replay["wire_attempts"][0]["loss_code"] = (
        "embedding_server_owner_unresponsive"
    )
    try:
        validate_replay_attempts(
            misclassified_replay,
            old_server_instance_id="server-old",
            new_server_instance_id="server-new",
        )
    except ProofFailure:
        pass
    else:
        raise ProofFailure(
            "an unresponsive-owner timeout was accepted as the disconnect loss"
        )
    unclassified_replay = copy.deepcopy(replay)
    del unclassified_replay["wire_attempts"][0]["loss_code"]
    try:
        validate_replay_attempts(
            unclassified_replay,
            old_server_instance_id="server-old",
            new_server_instance_id="server-new",
        )
    except ProofFailure:
        pass
    else:
        raise ProofFailure("a loss without a typed classification was accepted")
    mislabeled_replay = copy.deepcopy(replay)
    mislabeled_replay["wire_attempts"][1]["loss_code"] = (
        "embedding_server_connection_lost"
    )
    try:
        validate_replay_attempts(
            mislabeled_replay,
            old_server_instance_id="server-old",
            new_server_instance_id="server-new",
        )
    except ProofFailure:
        pass
    else:
        raise ProofFailure("a completed attempt carrying a loss code was accepted")
