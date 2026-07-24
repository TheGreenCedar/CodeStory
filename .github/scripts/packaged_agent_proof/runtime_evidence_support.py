"""Shared raw-evidence loading and metric comparison."""

from __future__ import annotations

import hashlib
import json
import stat
from pathlib import Path

from .foundation import ProofFailure, require


def metric_passes(
    value: int | float,
    threshold: int | float,
    comparison: str,
) -> bool:
    return {
        "equal": value == threshold,
        "greater_than_or_equal": value >= threshold,
        "less_than_or_equal": value <= threshold,
    }[comparison]


def load_external_raw_evidence(path: Path, label: str) -> tuple[dict, str]:
    require(
        path.is_file() and not path.is_symlink(),
        f"{label} is missing or unsafe: {path}",
    )
    metadata = path.stat()
    require(stat.S_ISREG(metadata.st_mode), f"{label} is not a regular file")
    require(
        metadata.st_size <= 8 * 1024 * 1024,
        f"{label} exceeds the 8 MiB evidence limit",
    )
    payload_bytes = path.read_bytes()
    try:
        payload = json.loads(payload_bytes)
    except json.JSONDecodeError as exc:
        raise ProofFailure(f"{label} is not valid JSON: {exc}") from exc
    require(isinstance(payload, dict), f"{label} must be an object")
    return payload, hashlib.sha256(payload_bytes).hexdigest()
