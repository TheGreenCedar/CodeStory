"""Typed publication-fault fixture, command, and run state."""

from __future__ import annotations

import subprocess
from dataclasses import dataclass
from pathlib import Path

@dataclass(frozen=True)
class PublicationFixture:
    project: Path
    anchors: list[str]
    source_file: Path
    lexical_file: Path
    baseline_source: str
    baseline_lexical: str
    file_times: dict[Path, tuple[int, int]]


@dataclass(frozen=True)
class PublicationCommands:
    run_id: str
    index: list[str]
    retrieval_index: list[str]
    status: list[str]


@dataclass(frozen=True)
class PublicationFaultRun:
    correlation_id: str
    pause_path: Path
    resume_path: Path
    snapshot_before: dict
    snapshot_after: dict
    returncode: int
    stdout: str
    stderr: str
    hook_events: list[dict]


@dataclass(frozen=True)
class PublicationCandidate:
    correlation_id: str
    nonce_sha256: str
    pause_path: Path
    resume_path: Path
    event_path: Path
    process: subprocess.Popen
