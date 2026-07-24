"""Worker-stall qualification assertions."""

from __future__ import annotations

from dataclasses import dataclass

from .contracts import require_nonnegative_int, require_positive_int, require_sha256
from .qualification_scenario_evidence import ScenarioAssertionEvidence, validate_replay_attempts

@dataclass(frozen=True)
class WorkerStallMeasurements:
    old_pid: int
    marker_sha256: str
    watchdog_observed_ns: int
    watchdog_last_progress_ns: int
    hard_no_progress_ms: int
    watchdog_cadence_ms: int


def _worker_stall_measurements(fail_stop: dict) -> WorkerStallMeasurements:
    require_nonnegative_int(
        fail_stop["watchdog_progress_sequence"],
        "worker stall watchdog progress sequence",
    )
    return WorkerStallMeasurements(
        old_pid=require_positive_int(
            fail_stop["old_pid"],
            "worker stall old pid",
        ),
        marker_sha256=require_sha256(
            fail_stop["watchdog_marker_sha256"],
            "worker stall watchdog marker",
        ),
        watchdog_observed_ns=require_nonnegative_int(
            fail_stop["watchdog_observed_ns"],
            "worker stall watchdog observed_ns",
        ),
        watchdog_last_progress_ns=require_nonnegative_int(
            fail_stop["watchdog_last_progress_ns"],
            "worker stall watchdog last progress",
        ),
        hard_no_progress_ms=require_positive_int(
            fail_stop["hard_native_no_progress_ms"],
            "worker stall hard no-progress bound",
        ),
        watchdog_cadence_ms=require_positive_int(
            fail_stop["watchdog_cadence_ms"],
            "worker stall watchdog cadence",
        ),
    )


def _worker_stall_assertions(
    evidence: ScenarioAssertionEvidence,
) -> dict[str, bool]:
    active = evidence.scheduler("stalled_request_observed")
    fail_stop = evidence.transition(
        "watchdog_fail_stop_observed",
        {
            "old_pid",
            "old_server_instance_id",
            "wire_attempt_count",
            "wire_attempts",
            "watchdog_marker_sha256",
            "watchdog_reason",
            "watchdog_observed_ns",
            "watchdog_last_progress_ns",
            "watchdog_progress_sequence",
            "hard_native_no_progress_ms",
            "watchdog_cadence_ms",
        },
    )
    replacement = evidence.transition(
        "post_stall_replacement",
        {"new_server_instance_id"},
    )
    survivor = evidence.transition(
        "unrelated_process_survived",
        {"pid", "process_start_id", "new_server_instance_id"},
    )
    measurements = _worker_stall_measurements(fail_stop)
    attempts = validate_replay_attempts(
        fail_stop,
        old_server_instance_id=fail_stop["old_server_instance_id"],
        new_server_instance_id=replacement["new_server_instance_id"],
    )
    replacement_observed = (
        replacement["new_server_instance_id"] in evidence.snapshot_instances
        and all(
            snapshot["process"]["pid"] != measurements.old_pid
            for snapshot in evidence.snapshots[-1:]
        )
    )
    return {
        "independent_watchdog_fail_stops_server": (
            active["active_request_class"] == "bulk"
            and bool(measurements.marker_sha256)
            and fail_stop["watchdog_reason"] == "embedding_engine_stalled"
            and measurements.watchdog_observed_ns
            >= measurements.watchdog_last_progress_ns
            and measurements.watchdog_observed_ns
            - measurements.watchdog_last_progress_ns
            >= measurements.hard_no_progress_ms * 1_000_000
            and measurements.watchdog_cadence_ms
            < measurements.hard_no_progress_ms
            and attempts[0].outcome == "server_loss"
            and replacement_observed
        ),
        "unrelated_process_survives": (
            replacement_observed
            and require_positive_int(
                survivor["pid"],
                "worker stall survivor pid",
            )
            != measurements.old_pid
            and bool(survivor["process_start_id"])
            and survivor["new_server_instance_id"]
            == replacement["new_server_instance_id"]
            and any(
                invocation.get("pid") == survivor["pid"]
                and invocation.get("process_start_id")
                == survivor["process_start_id"]
                and invocation.get("operation") == "query"
                and invocation.get("termination") == "exited"
                for invocation in evidence.invocations
            )
        ),
        "pure_embedding_rpc_replayed_at_most_once": (
            fail_stop["wire_attempt_count"] <= 2
            and sum(attempt.outcome == "completed" for attempt in attempts) == 1
        ),
    }
