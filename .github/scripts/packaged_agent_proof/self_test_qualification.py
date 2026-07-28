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
