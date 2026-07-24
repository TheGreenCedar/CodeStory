"""Private JSON document normalization for qualification evidence."""

from __future__ import annotations

import json
from dataclasses import dataclass
from pathlib import Path

from .foundation import (
    ProofFailure,
    require,
)


@dataclass(frozen=True)
class PrivateJsonMessages:
    missing_or_unsafe: str
    escaped: str
    leaked: str
    invalid_json: str
    non_object: str


@dataclass(frozen=True)
class PrivateJsonArtifact:
    name: str
    payload_bytes: bytes
    payload: dict


def _private_json_artifact(
    artifact_root: Path,
    name: str,
    *,
    forbidden_values: list[str],
    messages: PrivateJsonMessages,
) -> PrivateJsonArtifact:
    path = artifact_root / name
    require(
        path.is_file() and not path.is_symlink(),
        messages.missing_or_unsafe,
    )
    require(path.resolve().parent == artifact_root.resolve(), messages.escaped)
    payload_bytes = path.read_bytes()
    for forbidden in forbidden_values:
        require(
            forbidden.encode("utf-8") not in payload_bytes,
            messages.leaked,
        )
    try:
        payload = json.loads(payload_bytes)
    except json.JSONDecodeError as exc:
        raise ProofFailure(f"{messages.invalid_json}: {exc}") from exc
    require(isinstance(payload, dict), messages.non_object)
    return PrivateJsonArtifact(name, payload_bytes, payload)
