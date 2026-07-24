"""Typed state shared by packaged runtime proof phases."""

from __future__ import annotations

from dataclasses import dataclass
from pathlib import Path

from .process import McpProcess

@dataclass(frozen=True)
class RuntimeSetup:
    project_a: Path
    project_b: Path
    query_b: str
    plugin_root: Path
    provenance: dict | None
    node: Path
    qualified_env: dict[str, str]
    qualification_control: dict
    target_os: str
    command: list[str]
    embedded_models: Path


@dataclass(frozen=True)
class HostPair:
    host_a: McpProcess
    host_b: McpProcess
    start_a: str
    start_b: str


@dataclass(frozen=True)
class ColdProof:
    results: dict
    wall_ms: float
    ground_attempts: int
    identity_a: dict
    identity_b: dict
    snapshot_a: dict
    snapshot_b: dict
    shared_identity: dict
    status_a: dict
    status_b: dict


@dataclass(frozen=True)
class ContinuityProof:
    survivor: dict
    rejoin_snapshot: dict
    rejoin_identity: dict
