"""Client, owner, compatibility, and crash scenario assertions."""

from __future__ import annotations

from .contracts import require_nonnegative_int, require_positive_int
from .foundation import HEX_SHA256
from .qualification_scenario_evidence import ScenarioAssertionEvidence, validate_replay_attempts, validate_retry_state

def _client_death_assertions(
    evidence: ScenarioAssertionEvidence,
) -> dict[str, bool]:
    active = evidence.scheduler("dead_client_work_observed")
    continued = evidence.transition(
        "other_client_continued",
        {"project_identity_sha256"},
    )
    terminated = evidence.transition("client_terminated", {"termination"})
    reclaimed = evidence.scheduler("dead_client_work_reclaimed")
    post = evidence.transition(
        "post_reclaim_other_client_query",
        {"server_instance_id"},
    )
    return {
        "dead_client_queue_and_leases_reclaimed": (
            active["query_depth"] > 0
            and active["bulk_depth"] > 0
            and active["active_request_count"] > 0
            and active["lease_count"] > 0
            and reclaimed["query_depth"] == 0
            and reclaimed["bulk_depth"] == 0
            and reclaimed["active_request_count"] == 0
            and reclaimed["lease_count"] == 0
            and terminated["termination"] == "terminated"
        ),
        "other_client_continues": (
            HEX_SHA256.fullmatch(str(continued["project_identity_sha256"]))
            is not None
            and post["server_instance_id"] in evidence.snapshot_instances
        ),
        "no_server_replacement": len(evidence.snapshot_instances) == 1,
    }


def _frozen_owner_assertions(
    evidence: ScenarioAssertionEvidence,
) -> dict[str, bool]:
    bounded = evidence.transition(
        "bounded_owner_unresponsive",
        {
            "started_ns",
            "finished_ns",
            "error_code",
            "timeout_ms",
            "clock_domain",
            "clock_boot_id",
            "retry",
        },
    )
    stable = evidence.transition(
        "owner_identity_stable",
        {
            "server_instance_id",
            "lifetime_authority_id",
            "listener_id",
            "pid",
            "process_start_id",
            "post_release_query_succeeded",
        },
    )
    started = require_nonnegative_int(
        bounded["started_ns"],
        "frozen owner started_ns",
    )
    finished = require_nonnegative_int(
        bounded["finished_ns"],
        "frozen owner finished_ns",
    )
    timeout_ms = require_positive_int(
        bounded["timeout_ms"],
        "frozen owner timeout_ms",
    )
    stable_pid = require_positive_int(stable["pid"], "frozen owner stable pid")
    retry = validate_retry_state(bounded["retry"], "frozen owner retry")
    stable_identity = (
        len(evidence.snapshot_instances) == 1
        and stable["server_instance_id"] == next(iter(evidence.snapshot_instances))
        and len(evidence.snapshot_authorities) == 1
        and (
            stable["lifetime_authority_id"],
            stable["listener_id"],
        )
        == next(iter(evidence.snapshot_authorities))
        and all(
            snapshot["process"]["pid"] == stable_pid
            and snapshot["process"]["process_start_id"]
            == stable["process_start_id"]
            for snapshot in evidence.snapshots
        )
        and stable["post_release_query_succeeded"] is True
    )
    return {
        "owner_unresponsive_is_bounded": (
            finished >= started
            and finished - started <= timeout_ms * 1_000_000
            and bounded["clock_domain"] == "awake_monotonic"
            and bool(bounded["clock_boot_id"])
            and bounded["error_code"] == "embedding_server_owner_unresponsive"
            and retry.code == bounded["error_code"]
            and retry.retry_class == "after_server_change"
            and bool(retry.retry_condition)
        ),
        "authority_retained": stable_identity,
        "no_unlink": stable_identity,
        "no_pid_kill": stable_identity,
        "no_takeover": stable_identity,
        "no_second_engine": len(evidence.snapshot_engines) == 1,
    }


def _incompatible_owner_assertions(
    evidence: ScenarioAssertionEvidence,
) -> dict[str, bool]:
    active = evidence.transition(
        "active_owner_rejected",
        {"compatibility_evidence", "error_code", "retry"},
    )
    idle = evidence.transition(
        "idle_owner_draining",
        {"compatibility_evidence", "error_code", "retry"},
    )
    replacement = evidence.transition(
        "compatible_replacement",
        {"old_server_instance_id", "new_server_instance_id"},
    )
    replaced = (
        replacement["old_server_instance_id"]
        != replacement["new_server_instance_id"]
        and {
            replacement["old_server_instance_id"],
            replacement["new_server_instance_id"],
        }
        <= evidence.snapshot_instances
    )
    active_retry = validate_retry_state(
        active["retry"],
        "incompatible active retry",
    )
    idle_retry = validate_retry_state(
        idle["retry"],
        "incompatible idle retry",
    )
    expected_condition = "the incompatible server exits while fully idle"
    return {
        "idle_owner_drains": (
            idle["compatibility_evidence"] == "injected_contract_mismatch"
            and idle["error_code"] == "embedding_server_draining"
            and idle_retry.code == idle["error_code"]
            and idle_retry.retry_class == "after_owner_idle"
            and idle_retry.retry_after_ms == 0
            and idle_retry.retry_condition == expected_condition
            and replaced
        ),
        "active_owner_returns_typed_retry": (
            active["compatibility_evidence"] == "injected_contract_mismatch"
            and active["error_code"]
            == "embedding_server_incompatible_active_owner"
            and active_retry.code == active["error_code"]
            and active_retry.retry_class == "after_owner_idle"
            and active_retry.retry_after_ms == 0
            and active_retry.retry_condition == expected_condition
        ),
        "one_authority": len(evidence.snapshot_authorities) <= 2 and replaced,
        "one_engine_maximum": len(evidence.snapshot_instances) == 2 and replaced,
    }


def _server_crash_assertions(
    evidence: ScenarioAssertionEvidence,
) -> dict[str, bool]:
    active = evidence.scheduler("inflight_request_observed")
    replacement = evidence.transition(
        "server_replaced",
        {"old_server_instance_id", "new_server_instance_id"},
    )
    replay = evidence.transition(
        "query_replayed",
        {
            "logical_operation_count",
            "wire_attempt_count",
            "wire_attempts",
        },
    )
    attempts = validate_replay_attempts(
        replay,
        old_server_instance_id=replacement["old_server_instance_id"],
        new_server_instance_id=replacement["new_server_instance_id"],
    )
    return {
        "one_replacement_server": (
            active["active_request_class"] == "query"
            and replacement["old_server_instance_id"]
            != replacement["new_server_instance_id"]
            and [attempt.server_instance_id for attempt in attempts]
            == [
                replacement["old_server_instance_id"],
                replacement["new_server_instance_id"],
            ]
        ),
        "pure_embedding_rpc_replayed_at_most_once": (
            replay["logical_operation_count"] == 1
            and replay["wire_attempt_count"] <= 2
            and sum(attempt.outcome == "completed" for attempt in attempts) == 1
        ),
    }
