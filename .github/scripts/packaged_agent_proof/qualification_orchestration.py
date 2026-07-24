"""Qualification process-invocation and orchestration validation."""

from __future__ import annotations

from .contracts import require_exact_keys, require_nonempty_string, require_nonnegative_int, require_positive_int
from .foundation import require
from .qualification_artifact_types import QualificationOrchestration

def _qualification_invocations(
    value: object,
    *,
    name: str,
    started_ns: int,
    finished_ns: int,
) -> tuple[dict, ...]:
    require(
        isinstance(value, list),
        f"qualification artifact {name} process invocations are malformed",
    )
    invocation_ids: set[str] = set()
    for index, invocation in enumerate(value):
        field = f"qualification artifact {name} process invocation {index}"
        require(isinstance(invocation, dict), f"{field} is malformed")
        require_exact_keys(
            invocation,
            {
                "invocation_id",
                "operation",
                "project_identity_sha256",
                "pid",
                "process_start_id",
                "started_ns",
                "finished_ns",
                "exit_code",
                "termination",
            },
            field,
        )
        invocation_id = require_nonempty_string(
            invocation["invocation_id"],
            f"{field}.invocation_id",
        )
        require(
            invocation_id not in invocation_ids,
            f"qualification artifact {name} duplicated invocation {invocation_id}",
        )
        invocation_ids.add(invocation_id)
        require_nonempty_string(invocation["operation"], f"{field}.operation")
        require_sha256(
            invocation["project_identity_sha256"],
            f"{field}.project_identity_sha256",
        )
        require_positive_int(invocation["pid"], f"{field}.pid")
        require_nonempty_string(
            invocation["process_start_id"],
            f"{field}.process_start_id",
        )
        invocation_started = require_nonnegative_int(
            invocation["started_ns"],
            f"{field}.started_ns",
        )
        invocation_finished = require_nonnegative_int(
            invocation["finished_ns"],
            f"{field}.finished_ns",
        )
        require(
            started_ns <= invocation_started <= invocation_finished <= finished_ns,
            f"{field} escaped its block",
        )
        require(
            invocation["exit_code"] is None
            or (
                isinstance(invocation["exit_code"], int)
                and not isinstance(invocation["exit_code"], bool)
            ),
            f"{field}.exit_code is invalid",
        )
        require(
            invocation["termination"] in {"exited", "terminated"},
            f"{field}.termination is invalid",
        )
    return tuple(value)


def _qualification_orchestration(
    value: object,
    *,
    name: str,
) -> QualificationOrchestration:
    field = f"qualification artifact {name} orchestration"
    require(isinstance(value, dict), f"{field} is malformed")
    require_exact_keys(
        value,
        {"started_ns", "finished_ns", "process_invocations"},
        field,
    )
    started_ns = require_nonnegative_int(value["started_ns"], f"{field}.started_ns")
    finished_ns = require_nonnegative_int(value["finished_ns"], f"{field}.finished_ns")
    require(
        finished_ns >= started_ns,
        f"qualification artifact {name} orchestration moved backwards",
    )
    return QualificationOrchestration(
        started_ns,
        finished_ns,
        _qualification_invocations(
            value["process_invocations"],
            name=name,
            started_ns=started_ns,
            finished_ns=finished_ns,
        ),
    )
