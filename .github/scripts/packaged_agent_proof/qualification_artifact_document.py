"""Private qualification artifact document loading and summary validation."""

from __future__ import annotations

from pathlib import Path

from .contract_primitives import (
    require_exact_keys,
    require_nonempty_string,
    require_nonnegative_int,
)
from .foundation import require
from .qualification_artifact_types import QualificationArtifactSummary
from .qualification_documents import (
    PrivateJsonArtifact,
    PrivateJsonMessages,
    _private_json_artifact,
)


def _normalized_qualification_summary(
    summary: object,
    *,
    scenario_id: str,
) -> QualificationArtifactSummary:
    require(
        isinstance(summary, dict),
        f"qualification scenario {scenario_id} summary is malformed",
    )
    require_exact_keys(
        summary,
        {
            "artifact",
            "process_count",
            "control_event_count",
            "process_observation_count",
            "observation_count",
            "event_count",
        },
        f"qualification scenario {scenario_id} summary",
    )
    return QualificationArtifactSummary(
        require_nonempty_string(
            summary["artifact"],
            f"qualification scenario {scenario_id} artifact",
        ),
        require_nonnegative_int(
            summary["process_count"],
            f"qualification scenario {scenario_id} summary process_count",
        ),
        require_nonnegative_int(
            summary["control_event_count"],
            f"qualification scenario {scenario_id} summary control_event_count",
        ),
        require_nonnegative_int(
            summary["process_observation_count"],
            f"qualification scenario {scenario_id} summary process_observation_count",
        ),
        require_nonnegative_int(
            summary["observation_count"],
            f"qualification scenario {scenario_id} summary observation_count",
        ),
        require_nonnegative_int(
            summary["event_count"],
            f"qualification scenario {scenario_id} summary event_count",
        ),
    )


def _qualification_artifact_document(
    artifact_root: Path,
    summary: object,
    *,
    scenario_id: str,
    contracts: dict,
    forbidden_values: list[str],
) -> tuple[QualificationArtifactSummary, PrivateJsonArtifact]:
    normalized_summary = _normalized_qualification_summary(
        summary,
        scenario_id=scenario_id,
    )
    name = normalized_summary.name
    relative = Path(name)
    require(
        not relative.is_absolute()
        and len(relative.parts) == 1
        and relative.name == name
        and relative.suffix == ".json",
        f"qualification scenario {scenario_id} artifact must be a JSON basename",
    )
    document = _private_json_artifact(
        artifact_root,
        name,
        forbidden_values=forbidden_values,
        messages=PrivateJsonMessages(
            missing_or_unsafe=f"qualification artifact is missing or unsafe: {name}",
            escaped=(
                f"qualification artifact escaped its private output directory: {name}"
            ),
            leaked=f"qualification artifact {name} leaked private request material",
            invalid_json=f"qualification artifact {name} is not valid JSON",
            non_object=f"qualification artifact {name} must be an object",
        ),
    )
    payload = document.payload
    require_exact_keys(
        payload,
        {
            "schema_version",
            "scenario",
            "contracts",
            "orchestration",
            "control_events",
            "process_observations",
            "observations",
            "events",
        },
        f"qualification artifact {name}",
    )
    require(
        payload["schema_version"] == 3,
        f"qualification artifact {name} schema is unsupported",
    )
    require(
        payload["scenario"] == scenario_id,
        f"qualification artifact {name} names the wrong scenario",
    )
    require(
        payload["contracts"] == contracts,
        f"qualification artifact {name} used different contracts",
    )
    return normalized_summary, document
