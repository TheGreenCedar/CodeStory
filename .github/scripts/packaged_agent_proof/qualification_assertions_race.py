"""Cold-race qualification assertions."""

from __future__ import annotations

from dataclasses import dataclass

from .contract_primitives import require_positive_int
from .foundation import HEX_SHA256, require
from .qualification_scenario_evidence import ScenarioAssertionEvidence


@dataclass(frozen=True)
class ColdRaceElection:
    instances: frozenset[str]
    authorities: frozenset[tuple[str, str]]
    engines: frozenset[tuple[str, str, int, int]]


def _cold_race_election(
    evidence: ScenarioAssertionEvidence,
) -> ColdRaceElection:
    witnesses = {
        phase: [
            observation
            for observation in evidence.process_observations
            if observation.get("phase") == phase
        ]
        for phase in ("cold_race_first", "cold_race_second")
    }
    require(
        all(len(phase_witnesses) == 1 for phase_witnesses in witnesses.values()),
        "cold race must retain exactly one post-reset snapshot from each process",
    )
    snapshots = tuple(
        witnesses[phase][0]["snapshot"]
        for phase in ("cold_race_first", "cold_race_second")
    )
    require(
        all(
            isinstance(snapshot, dict) and snapshot.get("engine") is not None
            for snapshot in snapshots
        ),
        "cold race post-reset snapshots must retain engine identity",
    )
    return ColdRaceElection(
        instances=frozenset(
            snapshot["process"]["server_instance_id"] for snapshot in snapshots
        ),
        authorities=frozenset(
            (
                snapshot["authority"]["lifetime_authority_id"],
                snapshot["authority"]["listener_id"],
            )
            for snapshot in snapshots
        ),
        engines=frozenset(
            (
                snapshot["engine"]["engine_owner_id"],
                snapshot["engine"]["native_worker_id"],
                snapshot["engine"]["load_generation"],
                snapshot["engine"]["model_load_count"],
            )
            for snapshot in snapshots
        ),
    )


def _cold_race_assertions(
    evidence: ScenarioAssertionEvidence,
) -> dict[str, bool]:
    election = _cold_race_election(evidence)
    independent = evidence.transition(
        "two_independent_processes",
        {
            "first_pid",
            "second_pid",
            "first_project_identity_sha256",
            "second_project_identity_sha256",
            "first_transport_peer_verified",
            "second_transport_peer_verified",
        },
    )
    converged = evidence.transition(
        "single_server_convergence",
        {"server_instance_id", "lifetime_authority_id"},
    )
    hosts = (
        evidence.same_account.get("plugin_hosts")
        if isinstance(evidence.same_account, dict)
        else None
    )
    return {
        "two_independent_plugin_hosts": (
            require_positive_int(independent["first_pid"], "cold race first pid")
            != require_positive_int(independent["second_pid"], "cold race second pid")
            and independent["first_transport_peer_verified"] is True
            and independent["second_transport_peer_verified"] is True
        ),
        "same_os_account": (
            evidence.same_account.get("relation") == "same_os_account"
            and isinstance(hosts, list)
            and len(hosts) == 2
        ),
        "different_repositories": (
            independent["first_project_identity_sha256"]
            != independent["second_project_identity_sha256"]
            and all(
                HEX_SHA256.fullmatch(str(independent[field])) is not None
                for field in (
                    "first_project_identity_sha256",
                    "second_project_identity_sha256",
                )
            )
        ),
        "one_lifetime_authority": (
            len(election.authorities) == 1
            and converged["lifetime_authority_id"]
            == next(iter(election.authorities))[0]
        ),
        "one_listener": (len({identity[1] for identity in election.authorities}) == 1),
        "one_server": (
            len(election.instances) == 1
            and converged["server_instance_id"] == next(iter(election.instances))
        ),
        "one_engine_owner": (len({identity[0] for identity in election.engines}) == 1),
        "one_native_worker": (len({identity[1] for identity in election.engines}) == 1),
        "one_load_generation": (
            len({identity[2] for identity in election.engines}) == 1
        ),
        "one_model_load": (
            len(election.engines) == 1 and next(iter(election.engines))[3] == 1
        ),
    }
