"""Normalized runtime evidence for qualification scenarios."""

from __future__ import annotations

from dataclasses import dataclass

from .foundation import (
    require,
)
from .contracts import (
    require_exact_keys,
    require_nonempty_string,
    require_nonnegative_int,
)


@dataclass(frozen=True)
class RetryState:
    code: str
    message_head: str
    retry_class: str
    retry_after_ms: int
    retry_condition: str
    capacity: object | None


def validate_retry_state(value: object, field: str) -> RetryState:
    require(isinstance(value, dict), f"{field} is malformed")
    expected = {
        "code",
        "message_head",
        "retry_class",
        "retry_after_ms",
        "retry_condition",
    }
    if "capacity" in value:
        expected.add("capacity")
    require_exact_keys(value, expected, field)
    code = require_nonempty_string(value["code"], f"{field}.code")
    message_head = require_nonempty_string(
        value["message_head"],
        f"{field}.message_head",
    )
    retry_class = require_nonempty_string(
        value["retry_class"],
        f"{field}.retry_class",
    )
    retry_after_ms = require_nonnegative_int(
        value["retry_after_ms"],
        f"{field}.retry_after_ms",
    )
    retry_condition = require_nonempty_string(
        value["retry_condition"],
        f"{field}.retry_condition",
    )
    require(
        retry_class
        in {
            "after_capacity_change",
            "after_delay",
            "after_owner_idle",
            "after_server_change",
            "none",
            "same_rpc_once",
            "terminal",
        },
        f"{field}.retry_class is outside the protocol contract",
    )
    return RetryState(
        code=code,
        message_head=message_head,
        retry_class=retry_class,
        retry_after_ms=retry_after_ms,
        retry_condition=retry_condition,
        capacity=value.get("capacity"),
    )


@dataclass(frozen=True)
class ReplayAttempt:
    ordinal: int
    request_id: str
    server_instance_id: str
    submitted_ns: int
    completed_ns: int
    outcome: str


def _validated_replay_attempt(value: object, index: int) -> ReplayAttempt:
    require(isinstance(value, dict), "replay attempt is malformed")
    require_exact_keys(
        value,
        {
            "ordinal",
            "request_id",
            "server_instance_id",
            "submitted_ns",
            "completed_ns",
            "outcome",
        },
        f"replay attempt {index}",
    )
    require(value["ordinal"] == index, "replay attempt ordinal is not exact")
    request_id = require_nonempty_string(
        value["request_id"],
        "replay attempt request ID",
    )
    server_instance_id = require_nonempty_string(
        value["server_instance_id"],
        f"replay attempt {index} server_instance_id",
    )
    submitted_ns = require_nonnegative_int(
        value["submitted_ns"],
        f"replay attempt {index} submitted_ns",
    )
    completed_ns = require_nonnegative_int(
        value["completed_ns"],
        f"replay attempt {index} completed_ns",
    )
    require(completed_ns >= submitted_ns, "replay attempt clock moved backwards")
    outcome = require_nonempty_string(
        value["outcome"],
        f"replay attempt {index} outcome",
    )
    return ReplayAttempt(
        ordinal=index,
        request_id=request_id,
        server_instance_id=server_instance_id,
        submitted_ns=submitted_ns,
        completed_ns=completed_ns,
        outcome=outcome,
    )


def validate_replay_attempts(
    values: dict,
    *,
    old_server_instance_id: str,
    new_server_instance_id: str,
) -> tuple[ReplayAttempt, ReplayAttempt]:
    raw_attempts = values["wire_attempts"]
    require(
        values["wire_attempt_count"] == 2
        and isinstance(raw_attempts, list)
        and len(raw_attempts) == 2,
        "replay evidence must contain exactly the original RPC and one replay",
    )
    attempts = (
        _validated_replay_attempt(raw_attempts[0], 1),
        _validated_replay_attempt(raw_attempts[1], 2),
    )
    original, replay = attempts
    require(
        original.request_id != replay.request_id
        and original.server_instance_id == old_server_instance_id
        and original.outcome == "server_loss"
        and replay.server_instance_id == new_server_instance_id
        and replay.outcome == "completed",
        "replay attempts do not bind the old loss and exact replacement completion",
    )
    return attempts


@dataclass(frozen=True)
class ScenarioAssertionEvidence:
    scenario_id: str
    observations_by_kind: dict[str, list[dict]]
    process_observations: list[dict]
    invocations: list[dict]
    same_account: dict
    materialization: dict
    snapshots: tuple[dict, ...]
    snapshot_instances: frozenset[str]
    snapshot_authorities: frozenset[tuple[str, str]]
    snapshot_engines: frozenset[tuple[str, str, int, int]]

    @classmethod
    def from_raw(
        cls,
        scenario_id: str,
        *,
        observations_by_kind: dict[str, list[dict]],
        process_observations: list[dict],
        invocations: list[dict],
        same_account: dict,
        materialization: dict,
    ) -> ScenarioAssertionEvidence:
        snapshots = tuple(
            observation["snapshot"]
            for observation in process_observations
            if observation.get("snapshot") is not None
        )
        return cls(
            scenario_id=scenario_id,
            observations_by_kind=observations_by_kind,
            process_observations=process_observations,
            invocations=invocations,
            same_account=same_account,
            materialization=materialization,
            snapshots=snapshots,
            snapshot_instances=frozenset(
                snapshot["process"]["server_instance_id"]
                for snapshot in snapshots
            ),
            snapshot_authorities=frozenset(
                (
                    snapshot["authority"]["lifetime_authority_id"],
                    snapshot["authority"]["listener_id"],
                )
                for snapshot in snapshots
            ),
            snapshot_engines=frozenset(
                (
                    snapshot["engine"]["engine_owner_id"],
                    snapshot["engine"]["native_worker_id"],
                    snapshot["engine"]["load_generation"],
                    snapshot["engine"]["model_load_count"],
                )
                for snapshot in snapshots
                if snapshot.get("engine") is not None
            ),
        )

    def transition(
        self,
        kind: str,
        expected_keys: set[str] | None = None,
    ) -> dict:
        matches = self.observations_by_kind.get(kind, [])
        require(
            len(matches) == 1,
            f"qualification scenario {self.scenario_id} omitted or duplicated transition {kind}",
        )
        values = matches[0]["values"]
        require(
            isinstance(values, dict),
            f"qualification transition {kind} values are malformed",
        )
        if expected_keys is not None:
            require_exact_keys(
                values,
                expected_keys,
                f"qualification transition {kind} values",
            )
        return values

    def scheduler(self, kind: str) -> dict:
        values = self.transition(
            kind,
            {
                "query_capacity",
                "query_depth",
                "bulk_capacity",
                "bulk_depth",
                "active_request_count",
                "lease_count",
                "active_request_class",
            },
        )
        for field in (
            "query_capacity",
            "query_depth",
            "bulk_capacity",
            "bulk_depth",
            "active_request_count",
            "lease_count",
        ):
            require_nonnegative_int(
                values[field],
                f"qualification transition {kind}.{field}",
            )
        require(
            values["active_request_class"] in {None, "query", "bulk"},
            f"qualification transition {kind} has an invalid active request class",
        )
        return values
