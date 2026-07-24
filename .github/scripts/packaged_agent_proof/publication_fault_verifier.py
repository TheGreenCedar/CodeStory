"""Verification of publication-fault raw evidence."""

from __future__ import annotations

import re
from pathlib import Path

from .contract_primitives import (
    require_exact_keys,
    require_nonnegative_int,
    require_opaque_identifier,
    require_positive_int,
    require_sha256,
)
from .foundation import PUBLICATION_FAULT_EVIDENCE_CONTRACT, require
from .runtime_evidence_support import load_external_raw_evidence


def _verify_publication_header(
    payload: dict,
    *,
    source: dict,
    package: dict,
    contracts: dict,
) -> tuple[str, str]:
    require_exact_keys(
        payload,
        {
            "schema_version",
            "evidence_contract",
            "source",
            "package",
            "contracts",
            "correlation_id",
            "previous_publication_identity_sha256",
            "server_observations",
            "candidate_observation",
            "publication_hook_events",
            "ordinary_product_observations",
        },
        "publication fault raw evidence",
    )
    require(
        payload["schema_version"] == 1,
        "publication fault evidence schema is unsupported",
    )
    require(
        payload["evidence_contract"] == PUBLICATION_FAULT_EVIDENCE_CONTRACT,
        "publication fault evidence contract is unsupported",
    )
    require(
        payload["source"] == source,
        "publication fault evidence source identity is stale",
    )
    require(
        payload["package"] == package,
        "publication fault evidence package identity is stale",
    )
    require(
        payload["contracts"] == contracts,
        "publication fault evidence contracts are stale",
    )
    correlation_id = payload["correlation_id"]
    require(
        isinstance(correlation_id, str)
        and re.fullmatch(r"[0-9a-f]{32}", correlation_id) is not None,
        "publication fault correlation id is invalid",
    )
    previous_publication = require_sha256(
        payload["previous_publication_identity_sha256"],
        "publication fault previous publication identity",
    )
    return correlation_id, previous_publication


def _verify_server_observations(observations: object) -> None:
    require(
        isinstance(observations, list) and len(observations) == 2,
        "publication fault evidence requires before-crash and after-replacement "
        "server observations",
    )
    for index, (observation, phase) in enumerate(
        zip(observations, ("before_crash", "after_replacement"))
    ):
        require(
            isinstance(observation, dict), f"server observation {index} is malformed"
        )
        require_exact_keys(
            observation,
            {"phase", "server_instance_id", "process_start_id", "load_generation"},
            f"server observation {index}",
        )
        require(
            observation["phase"] == phase,
            f"server observation {index} has the wrong phase",
        )
        require_opaque_identifier(
            observation["server_instance_id"],
            f"server observation {index}.server_instance_id",
        )
        require_opaque_identifier(
            observation["process_start_id"],
            f"server observation {index}.process_start_id",
        )
        require_positive_int(
            observation["load_generation"],
            f"server observation {index}.load_generation",
        )
    require(
        (
            observations[0]["server_instance_id"],
            observations[0]["process_start_id"],
        )
        != (
            observations[1]["server_instance_id"],
            observations[1]["process_start_id"],
        ),
        "publication fault evidence did not observe a replacement server",
    )


def _verify_candidate_observation(candidate: object) -> None:
    require(
        isinstance(candidate, dict), "publication candidate observation is malformed"
    )
    require_exact_keys(
        candidate,
        {"command", "exit_code", "stdout_sha256", "stderr_sha256"},
        "publication candidate observation",
    )
    require(
        candidate["command"] == "retrieval_index",
        "publication candidate used the wrong command",
    )
    require(
        isinstance(candidate["exit_code"], int)
        and not isinstance(candidate["exit_code"], bool)
        and candidate["exit_code"] != 0,
        "publication candidate unexpectedly committed successfully",
    )
    require_sha256(candidate["stdout_sha256"], "publication candidate stdout sha256")
    require_sha256(candidate["stderr_sha256"], "publication candidate stderr sha256")


def _verify_hook_events(events: object, correlation_id: str) -> None:
    expected_events = (
        ("pause_before_manifest_commit", "waiting_for_resume"),
        ("resume_manifest_commit", "observed"),
        ("lease_revalidation", "failed"),
        ("manifest_commit", "returned_error"),
    )
    require(
        isinstance(events, list) and len(events) == len(expected_events),
        "publication hook evidence must contain the exact four raw fence events",
    )
    last_elapsed = -1
    for index, (event, expected) in enumerate(zip(events, expected_events)):
        require(isinstance(event, dict), f"publication hook event {index} is malformed")
        require_exact_keys(
            event,
            {
                "schema_version",
                "sequence",
                "correlation_id",
                "action",
                "status",
                "clock",
            },
            f"publication hook event {index}",
        )
        require(
            event["schema_version"] == 1,
            f"publication hook event {index} schema is unsupported",
        )
        require(
            event["sequence"] == index, "publication hook event sequence is not exact"
        )
        require(
            event["correlation_id"] == correlation_id,
            "publication hook correlation changed",
        )
        require(
            (event["action"], event["status"]) == expected,
            f"publication hook event {index} does not match the fence contract",
        )
        clock = event["clock"]
        require(
            isinstance(clock, dict), f"publication hook event {index} omitted its clock"
        )
        require_exact_keys(
            clock,
            {"domain", "api", "elapsed_ns"},
            f"publication hook event {index} clock",
        )
        require(
            clock["domain"] == "process_monotonic"
            and clock["api"] == "std::time::Instant",
            "publication hook used an unsupported clock",
        )
        elapsed = require_nonnegative_int(
            clock["elapsed_ns"],
            f"publication hook event {index} elapsed_ns",
        )
        require(
            elapsed >= last_elapsed, "publication hook elapsed time moved backwards"
        )
        last_elapsed = elapsed


def _verify_product_observations(
    observations: object,
    previous_publication: str,
) -> None:
    require(
        isinstance(observations, list) and len(observations) == 2,
        "publication fault evidence requires status and query product observations",
    )
    for index, (observation, command) in enumerate(
        zip(observations, ("retrieval_status", "search"))
    ):
        require(
            isinstance(observation, dict),
            f"ordinary product observation {index} is malformed",
        )
        require_exact_keys(
            observation,
            {
                "sequence",
                "command",
                "exit_code",
                "retrieval_mode",
                "publication_identity_sha256",
                "output_sha256",
            },
            f"ordinary product observation {index}",
        )
        require(
            observation["sequence"] == index,
            "ordinary product observation order changed",
        )
        require(
            observation["command"] == command,
            "ordinary product observation used the wrong command",
        )
        require(observation["exit_code"] == 0, f"ordinary product {command} failed")
        require(
            observation["retrieval_mode"] == "full",
            f"ordinary product {command} was not full",
        )
        require(
            require_sha256(
                observation["publication_identity_sha256"],
                f"ordinary product {command} publication identity",
            )
            == previous_publication,
            f"ordinary product {command} did not use the previous publication",
        )
        require_sha256(
            observation["output_sha256"],
            f"ordinary product {command} output sha256",
        )


def verify_publication_fault_raw_evidence(
    path: Path,
    *,
    source: dict,
    package: dict,
    contracts: dict,
) -> dict:
    payload, artifact_sha256 = load_external_raw_evidence(
        path,
        "publication fault raw evidence",
    )
    correlation_id, previous_publication = _verify_publication_header(
        payload,
        source=source,
        package=package,
        contracts=contracts,
    )
    _verify_server_observations(payload["server_observations"])
    _verify_candidate_observation(payload["candidate_observation"])
    _verify_hook_events(payload["publication_hook_events"], correlation_id)
    _verify_product_observations(
        payload["ordinary_product_observations"],
        previous_publication,
    )
    return {
        "artifact": {
            "name": "publication-fault-external.raw.json",
            "sha256": artifact_sha256,
        },
        "assertions": {
            "lost_publication_lease_blocks_commit": True,
            "previous_publication_remains_usable": True,
        },
    }
