"""Raw scenario artifact normalization for qualification evidence."""

from __future__ import annotations

import hashlib
from pathlib import Path

from .contracts import require_nonempty_string
from .foundation import CANDIDATE_QUALIFICATION_MATRIX_ALIASES, require
from .qualification_artifact_document import _qualification_artifact_document
from .qualification_artifact_snapshots import _qualification_controls, _qualification_process_observations
from .qualification_artifact_transitions import _qualification_events, _qualification_transitions, _verify_scenario_artifact_requirements
from .qualification_artifact_types import QualificationArtifactEvidence
from .qualification_orchestration import _qualification_orchestration
from .qualification_scenarios import derive_scenario_assertions

def _verify_qualification_summary(
    *,
    scenario_id: str,
    evidence: QualificationArtifactEvidence,
) -> None:
    expected_counts = {
        "process_count": (
            evidence.summary.process_count,
            len(evidence.orchestration.invocations),
        ),
        "control_event_count": (
            evidence.summary.control_event_count,
            len(evidence.controls.events),
        ),
        "process_observation_count": (
            evidence.summary.process_observation_count,
            len(evidence.process_observations),
        ),
        "observation_count": (
            evidence.summary.observation_count,
            len(evidence.transitions.observations),
        ),
        "event_count": (evidence.summary.event_count, len(evidence.events)),
    }
    for field, (retained, expected) in expected_counts.items():
        require(
            retained == expected,
            f"qualification scenario {scenario_id} summary {field} is stale",
        )


def qualification_artifact(
    artifact_root: Path,
    summary: object,
    *,
    scenario_id: str,
    contracts: dict,
    package: dict,
    same_account: dict,
    materialization: dict,
    nonce_sha256: str,
    forbidden_values: list[str],
) -> tuple[dict, dict]:
    normalized_summary, document = _qualification_artifact_document(
        artifact_root,
        summary,
        scenario_id=scenario_id,
        contracts=contracts,
        forbidden_values=forbidden_values,
    )
    payload = document.payload
    orchestration = _qualification_orchestration(
        payload["orchestration"],
        name=document.name,
    )
    controls = _qualification_controls(
        payload["control_events"],
        name=document.name,
        contracts=contracts,
        package=package,
        nonce_sha256=nonce_sha256,
    )
    process_observations = _qualification_process_observations(
        payload["process_observations"],
        name=document.name,
        orchestration=orchestration,
        contracts=contracts,
        package=package,
    )
    transitions = _qualification_transitions(
        payload["observations"],
        name=document.name,
        orchestration=orchestration,
    )
    _verify_scenario_artifact_requirements(
        name=document.name,
        scenario_id=scenario_id,
        controls=controls,
        process_observations=process_observations,
        transitions=transitions,
    )
    evidence = QualificationArtifactEvidence(
        normalized_summary,
        document,
        orchestration,
        controls,
        process_observations,
        transitions,
        _qualification_events(
            payload["events"],
            name=document.name,
            orchestration=orchestration,
        ),
    )
    _verify_qualification_summary(
        scenario_id=scenario_id,
        evidence=evidence,
    )
    assertions = derive_scenario_assertions(
        scenario_id,
        observations_by_kind=evidence.transitions.by_kind,
        process_observations=list(evidence.process_observations),
        invocations=list(evidence.orchestration.invocations),
        same_account=same_account,
        materialization=materialization,
    )
    return (
        {
            "name": document.name,
            "sha256": hashlib.sha256(document.payload_bytes).hexdigest(),
        },
        assertions,
    )


def require_candidate_matrix_installation_source(
    cell_id: str | None,
    installation_source: str,
) -> None:
    alias = CANDIDATE_QUALIFICATION_MATRIX_ALIASES.get(cell_id)
    if alias is not None:
        require(
            installation_source == alias["installation_source"],
            "candidate qualification matrix alias requires candidate-installed provenance",
        )
