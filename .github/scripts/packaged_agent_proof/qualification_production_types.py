"""Typed state passed through qualification production phases."""

from __future__ import annotations

import argparse
from dataclasses import dataclass
from pathlib import Path


@dataclass(frozen=True)
class QualificationProducerContext:
    args: argparse.Namespace
    qualification_driver: Path
    qualification_cli: Path
    root: Path
    runtime: dict
    manifest: dict
    archive_sha256: str
    measurement_contract: dict
    private_root: Path
    artifact_root: Path
    nonce: str
    nonce_sha256: str
    projects: tuple[str, str]
    contracts: dict
    package: dict
    qualification_env: dict[str, str]
    server_cleanup_control: dict

    @property
    def forbidden_values(self) -> list[str]:
        return [self.nonce, *self.projects]


@dataclass(frozen=True)
class QualificationExternalEvidence:
    publication_fault: dict
    fault_recovery_consistency: dict | None
    retrieval_quality: dict | None


@dataclass(frozen=True)
class QualificationRunnerEvidence:
    output: dict
    expected_status: str
    expected_backend: str
    matrix_cell_id: str
    matrix_cell: dict


@dataclass(frozen=True)
class QualificationScenarioEvidence:
    shared_identity: dict
    scenarios: dict[str, dict]
