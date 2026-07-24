"""Typed fixtures shared by packaged proof full-stack self-tests."""

from __future__ import annotations

from dataclasses import dataclass
from pathlib import Path


@dataclass(frozen=True)
class FullStackFixture:
    root: Path
    measurement_protocol: Path
    binary_payload: bytes
    build_identity: str
    protocol_sha256: str
    constant_set_sha256: str
    measurement_protocol_sha256: str
    manifest: dict
    valid_manifest: dict


@dataclass(frozen=True)
class ServerIdentityFixture:
    snapshot_payload: dict
    first_snapshot: dict
    shared: dict
    valid_engine_identity: dict


@dataclass(frozen=True)
class TrueIdleFixture:
    transitions: dict
    process_observations: list[dict]
    invocations: list[dict]
    materialized_sha256: str


@dataclass(frozen=True)
class CalibrationFixture:
    bundle_path: Path
    bundle_payload: dict
    frozen_measurement_contract: dict


@dataclass(frozen=True)
class PublicationFixture:
    path: Path
    payload: dict
    external: dict


@dataclass(frozen=True)
class QualityFixture:
    path: Path
    payload: dict
    external: dict


@dataclass(frozen=True)
class ExternalEvidenceFixture:
    publication: dict
    consistency: dict
    quality: dict
