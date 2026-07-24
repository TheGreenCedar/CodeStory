"""Verification of post-fault rank-consistency evidence."""

from __future__ import annotations

from pathlib import Path

from .contracts import require_exact_keys, require_sha256
from .foundation import (
    FAULT_RECOVERY_CONSISTENCY_CASES,
    FAULT_RECOVERY_CONSISTENCY_CONTRACT,
    require,
)
from .runtime_evidence_support import load_external_raw_evidence


def _verify_consistency_observation(
    observation: object,
    *,
    index: int,
    case_ids: set[str],
) -> None:
    require(
        isinstance(observation, dict),
        f"fault recovery consistency observation {index} is malformed",
    )
    require_exact_keys(
        observation,
        {
            "case_id_sha256",
            "before_server_fault_rank",
            "after_server_replacement_rank",
        },
        f"fault recovery consistency observation {index}",
    )
    case_id = require_sha256(
        observation["case_id_sha256"],
        f"fault recovery consistency observation {index} case id",
    )
    require(
        case_id not in case_ids,
        "fault recovery consistency evidence contains duplicate cases",
    )
    case_ids.add(case_id)
    for field in ("before_server_fault_rank", "after_server_replacement_rank"):
        rank = observation[field]
        require(
            rank is None
            or (
                isinstance(rank, int) and not isinstance(rank, bool) and 1 <= rank <= 10
            ),
            f"fault recovery consistency observation {index} {field} "
            "is not a rank in the fixed top 10",
        )
    require(
        observation["before_server_fault_rank"]
        == observation["after_server_replacement_rank"],
        "fault recovery changed a search rank from the retained publication",
    )


def verify_fault_recovery_consistency_raw_evidence(
    path: Path,
    *,
    source: dict,
    package: dict,
    contracts: dict,
) -> dict:
    payload, artifact_sha256 = load_external_raw_evidence(
        path,
        "fault recovery consistency raw evidence",
    )
    require_exact_keys(
        payload,
        {
            "schema_version",
            "evidence_contract",
            "source",
            "package",
            "contracts",
            "run_id_sha256",
            "observations",
        },
        "fault recovery consistency raw evidence",
    )
    require(
        payload["schema_version"] == 1,
        "fault recovery consistency evidence schema is unsupported",
    )
    require(
        payload["evidence_contract"] == FAULT_RECOVERY_CONSISTENCY_CONTRACT,
        "fault recovery consistency evidence contract is unsupported",
    )
    require(
        payload["source"] == source,
        "fault recovery consistency source identity is stale",
    )
    require(
        payload["package"] == package,
        "fault recovery consistency package identity is stale",
    )
    require(
        payload["contracts"] == contracts,
        "fault recovery consistency contracts are stale",
    )
    require_sha256(payload["run_id_sha256"], "fault recovery consistency run id")
    observations = payload["observations"]
    require(
        isinstance(observations, list)
        and len(observations) == FAULT_RECOVERY_CONSISTENCY_CASES,
        "fault recovery consistency evidence has the wrong case count",
    )
    case_ids: set[str] = set()
    for index, observation in enumerate(observations):
        _verify_consistency_observation(
            observation,
            index=index,
            case_ids=case_ids,
        )
    return {
        "artifact": {
            "name": "fault-recovery-consistency.raw.json",
            "sha256": artifact_sha256,
        },
        "case_count": len(observations),
        "ranks_stable": True,
    }
