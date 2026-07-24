"""Self-tests for qualification phase contracts."""

from __future__ import annotations

import copy

from .foundation import ProofFailure, require
from .qualification_scenario_evidence import validate_replay_attempts, validate_retry_state


def _replay_attempt(
    ordinal: int,
    request_id: str,
    server_instance_id: str,
    outcome: str,
) -> dict[str, object]:
    return {
        "ordinal": ordinal,
        "request_id": request_id,
        "server_instance_id": server_instance_id,
        "submitted_ns": ordinal * 10,
        "completed_ns": ordinal * 10 + 1,
        "outcome": outcome,
    }


def run_qualification_self_tests() -> None:
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
            _replay_attempt(1, "request-1", "server-old", "server_loss"),
            _replay_attempt(2, "request-2", "server-new", "completed"),
        ],
    }
    attempts = validate_replay_attempts(
        replay,
        old_server_instance_id="server-old",
        new_server_instance_id="server-new",
    )
    require(
        attempts[0].outcome == "server_loss" and attempts[1].outcome == "completed",
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
