"""Publication-fault product observations and evidence payloads."""

from __future__ import annotations

import hashlib
from pathlib import Path

from .foundation import FAULT_RECOVERY_CONSISTENCY_CONTRACT, PUBLICATION_FAULT_EVIDENCE_CONTRACT, require
from .process import json_command
from .publication_fault_types import PublicationCommands, PublicationFaultRun, PublicationFixture
from .publication_protocol import publication_identity_from_status, run_quality_search, server_observation_from_control_event

def _post_fault_observations(
    cli: Path,
    env: dict[str, str],
    fixture: PublicationFixture,
    commands: PublicationCommands,
    *,
    timeout: int,
) -> tuple[dict, str, list[int | None], str, str]:
    status_result, status = json_command(
        commands.status,
        env=env,
        cwd=fixture.project,
        timeout=timeout,
    )
    publication = publication_identity_from_status(status)
    ranks = []
    first_search_sha256 = None
    for anchor in fixture.anchors:
        rank, output_sha256 = run_quality_search(
            cli,
            env,
            fixture.project,
            commands.run_id,
            anchor,
            anchor,
            timeout=timeout,
        )
        ranks.append(rank)
        if first_search_sha256 is None:
            first_search_sha256 = output_sha256
    require(first_search_sha256 is not None, "qualification search emitted no output digest")
    status_sha256 = hashlib.sha256(
        status_result["stdout"].encode("utf-8")
    ).hexdigest()
    return status, publication, ranks, status_sha256, first_search_sha256


def _publication_payload(
    fault: PublicationFaultRun,
    *,
    source: dict,
    package: dict,
    contracts: dict,
    previous_publication: str,
    final_status: dict,
    final_publication: str,
    status_sha256: str,
    search_sha256: str,
) -> dict:
    return {
        "schema_version": 1,
        "evidence_contract": PUBLICATION_FAULT_EVIDENCE_CONTRACT,
        "source": source,
        "package": package,
        "contracts": contracts,
        "correlation_id": fault.correlation_id,
        "previous_publication_identity_sha256": previous_publication,
        "server_observations": [
            server_observation_from_control_event(
                fault.snapshot_before,
                "before_crash",
            ),
            server_observation_from_control_event(
                fault.snapshot_after,
                "after_replacement",
            ),
        ],
        "candidate_observation": {
            "command": "retrieval_index",
            "exit_code": fault.returncode,
            "stdout_sha256": hashlib.sha256(fault.stdout.encode("utf-8")).hexdigest(),
            "stderr_sha256": hashlib.sha256(fault.stderr.encode("utf-8")).hexdigest(),
        },
        "publication_hook_events": fault.hook_events,
        "ordinary_product_observations": [
            {
                "sequence": 0,
                "command": "retrieval_status",
                "exit_code": 0,
                "retrieval_mode": final_status["retrieval_mode"],
                "publication_identity_sha256": final_publication,
                "output_sha256": status_sha256,
            },
            {
                "sequence": 1,
                "command": "search",
                "exit_code": 0,
                "retrieval_mode": final_status["retrieval_mode"],
                "publication_identity_sha256": final_publication,
                "output_sha256": search_sha256,
            },
        ],
    }


def _consistency_payload(
    fixture: PublicationFixture,
    fault: PublicationFaultRun,
    baseline_ranks: list[int | None],
    post_ranks: list[int | None],
    *,
    source: dict,
    package: dict,
    contracts: dict,
) -> dict:
    return {
        "schema_version": 1,
        "evidence_contract": FAULT_RECOVERY_CONSISTENCY_CONTRACT,
        "source": source,
        "package": package,
        "contracts": contracts,
        "run_id_sha256": hashlib.sha256(
            fault.correlation_id.encode("ascii")
        ).hexdigest(),
        "observations": [
            {
                "case_id_sha256": hashlib.sha256(anchor.encode("utf-8")).hexdigest(),
                "before_server_fault_rank": baseline_ranks[index],
                "after_server_replacement_rank": post_ranks[index],
            }
            for index, anchor in enumerate(fixture.anchors)
        ],
    }
