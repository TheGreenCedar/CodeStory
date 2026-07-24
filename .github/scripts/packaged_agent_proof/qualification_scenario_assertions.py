"""Dispatch qualification evidence to its scenario assertion owner."""

from __future__ import annotations

from .foundation import require
from .qualification_assertions_idle import _true_idle_assertions
from .qualification_assertions_owner import _client_death_assertions, _frozen_owner_assertions, _incompatible_owner_assertions, _server_crash_assertions
from .qualification_assertions_queue import _mixed_queue_assertions
from .qualification_assertions_race import _cold_race_assertions
from .qualification_assertions_stall import _worker_stall_assertions
from .qualification_scenario_evidence import ScenarioAssertionEvidence

def derive_scenario_assertions(
    scenario_id: str,
    *,
    observations_by_kind: dict[str, list[dict]],
    process_observations: list[dict],
    invocations: list[dict],
    same_account: dict,
    materialization: dict,
) -> dict[str, bool]:
    evidence = ScenarioAssertionEvidence.from_raw(
        scenario_id,
        observations_by_kind=observations_by_kind,
        process_observations=process_observations,
        invocations=invocations,
        same_account=same_account,
        materialization=materialization,
    )
    handler = {
        "client_death": _client_death_assertions,
        "cold_race": _cold_race_assertions,
        "frozen_owner": _frozen_owner_assertions,
        "incompatible_owner": _incompatible_owner_assertions,
        "mixed_queue": _mixed_queue_assertions,
        "server_crash": _server_crash_assertions,
        "true_idle_respawn": _true_idle_assertions,
        "worker_stall": _worker_stall_assertions,
    }.get(scenario_id)
    require(handler is not None, f"unknown qualification scenario {scenario_id}")
    assertions = handler(evidence)
    failed = sorted(name for name, value in assertions.items() if value is not True)
    require(
        not failed,
        f"qualification scenario {scenario_id} raw evidence failed assertions: {', '.join(failed)}",
    )
    return assertions
