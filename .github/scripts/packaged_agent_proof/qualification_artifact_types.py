"""Typed normalized evidence for one qualification artifact."""

from __future__ import annotations

from dataclasses import dataclass

from .qualification_documents import PrivateJsonArtifact

@dataclass(frozen=True)
class QualificationArtifactSummary:
    name: str
    process_count: int
    control_event_count: int
    process_observation_count: int
    observation_count: int
    event_count: int


@dataclass(frozen=True)
class QualificationOrchestration:
    started_ns: int
    finished_ns: int
    invocations: tuple[dict, ...]


@dataclass(frozen=True)
class QualificationControlEvidence:
    events: tuple[dict, ...]
    actions: tuple[str, ...]


@dataclass(frozen=True)
class QualificationTransitionEvidence:
    observations: tuple[dict, ...]
    by_kind: dict[str, list[dict]]


@dataclass(frozen=True)
class QualificationArtifactEvidence:
    summary: QualificationArtifactSummary
    document: PrivateJsonArtifact
    orchestration: QualificationOrchestration
    controls: QualificationControlEvidence
    process_observations: tuple[dict, ...]
    transitions: QualificationTransitionEvidence
    events: tuple[dict, ...]
