"""Raw scenario artifact normalization for qualification evidence."""

from __future__ import annotations

import hashlib
from collections import Counter
from dataclasses import dataclass
from pathlib import Path

from .foundation import (
    CANDIDATE_QUALIFICATION_MATRIX_ALIASES,
    SERVER_LIFECYCLES,
    require,
)
from .contracts import (
    require_exact_keys,
    require_nonempty_string,
    require_nonnegative_int,
    require_positive_int,
    require_sha256,
)
from .qualification_scenarios import derive_scenario_assertions
from .qualification_documents import (
    PrivateJsonArtifact,
    PrivateJsonMessages,
    _private_json_artifact,
)


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
                "qualification artifact escaped its private output directory: "
                f"{name}"
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


def _qualification_invocations(
    value: object,
    *,
    name: str,
    started_ns: int,
    finished_ns: int,
) -> tuple[dict, ...]:
    require(
        isinstance(value, list),
        f"qualification artifact {name} process invocations are malformed",
    )
    invocation_ids: set[str] = set()
    for index, invocation in enumerate(value):
        field = f"qualification artifact {name} process invocation {index}"
        require(isinstance(invocation, dict), f"{field} is malformed")
        require_exact_keys(
            invocation,
            {
                "invocation_id",
                "operation",
                "project_identity_sha256",
                "pid",
                "process_start_id",
                "started_ns",
                "finished_ns",
                "exit_code",
                "termination",
            },
            field,
        )
        invocation_id = require_nonempty_string(
            invocation["invocation_id"],
            f"{field}.invocation_id",
        )
        require(
            invocation_id not in invocation_ids,
            f"qualification artifact {name} duplicated invocation {invocation_id}",
        )
        invocation_ids.add(invocation_id)
        require_nonempty_string(invocation["operation"], f"{field}.operation")
        require_sha256(
            invocation["project_identity_sha256"],
            f"{field}.project_identity_sha256",
        )
        require_positive_int(invocation["pid"], f"{field}.pid")
        require_nonempty_string(
            invocation["process_start_id"],
            f"{field}.process_start_id",
        )
        invocation_started = require_nonnegative_int(
            invocation["started_ns"],
            f"{field}.started_ns",
        )
        invocation_finished = require_nonnegative_int(
            invocation["finished_ns"],
            f"{field}.finished_ns",
        )
        require(
            started_ns <= invocation_started <= invocation_finished <= finished_ns,
            f"{field} escaped its block",
        )
        require(
            invocation["exit_code"] is None
            or (
                isinstance(invocation["exit_code"], int)
                and not isinstance(invocation["exit_code"], bool)
            ),
            f"{field}.exit_code is invalid",
        )
        require(
            invocation["termination"] in {"exited", "terminated"},
            f"{field}.termination is invalid",
        )
    return tuple(value)


def _qualification_orchestration(
    value: object,
    *,
    name: str,
) -> QualificationOrchestration:
    field = f"qualification artifact {name} orchestration"
    require(isinstance(value, dict), f"{field} is malformed")
    require_exact_keys(
        value,
        {"started_ns", "finished_ns", "process_invocations"},
        field,
    )
    started_ns = require_nonnegative_int(value["started_ns"], f"{field}.started_ns")
    finished_ns = require_nonnegative_int(value["finished_ns"], f"{field}.finished_ns")
    require(
        finished_ns >= started_ns,
        f"qualification artifact {name} orchestration moved backwards",
    )
    return QualificationOrchestration(
        started_ns,
        finished_ns,
        _qualification_invocations(
            value["process_invocations"],
            name=name,
            started_ns=started_ns,
            finished_ns=finished_ns,
        ),
    )


def _validate_qualification_clock(
    clock: object,
    field: str,
    *,
    observed: bool,
) -> dict:
    require(isinstance(clock, dict), f"{field} is malformed")
    numeric = "observed_ns" if observed else "resolution_ns"
    require_exact_keys(clock, {"domain", "api", "boot_id", numeric}, field)
    require(
        clock["domain"] == "awake_monotonic",
        f"{field} used the wrong clock domain",
    )
    require_nonempty_string(clock["api"], f"{field}.api")
    require_nonempty_string(clock["boot_id"], f"{field}.boot_id")
    require_nonnegative_int(clock[numeric], f"{field}.{numeric}")
    return clock


def _validate_snapshot_identity(
    snapshot: dict,
    *,
    field: str,
    package: dict,
) -> None:
    authority = snapshot["authority"]
    process = snapshot["process"]
    scheduler = snapshot["scheduler"]
    require(
        isinstance(authority, dict)
        and isinstance(process, dict)
        and isinstance(scheduler, dict),
        f"{field} omitted server identity",
    )
    require_exact_keys(
        process,
        {
            "server_instance_id",
            "pid",
            "process_start_id",
            "executable_sha256",
            "executable_version",
        },
        f"{field}.process",
    )
    for identity_field in (
        "endpoint_namespace_id",
        "lifetime_authority_id",
        "listener_id",
    ):
        require_nonempty_string(
            authority.get(identity_field),
            f"{field}.authority.{identity_field}",
        )
    require_nonempty_string(
        process.get("server_instance_id"),
        f"{field}.process.server_instance_id",
    )
    require_positive_int(process.get("pid"), f"{field}.process.pid")
    require_nonempty_string(
        process.get("process_start_id"),
        f"{field}.process.process_start_id",
    )
    require(
        process.get("executable_sha256") == package["executable_sha256"]
        and process.get("executable_version") == package["release_version"],
        f"{field}.process does not match the exact packaged executable",
    )
    require(
        scheduler.get("query_capacity") == 64
        and scheduler.get("bulk_capacity") == 64,
        f"{field} queue capacities differ from the bound contract",
    )


def _validated_qualification_snapshot(
    snapshot: object,
    *,
    field: str,
    contracts: dict,
    package: dict,
) -> dict:
    require(isinstance(snapshot, dict), f"{field} is malformed")
    required = {
        "schema_version",
        "event_sequence",
        "lifecycle",
        "clock",
        "protocol",
        "authority",
        "process",
        "scheduler",
    }
    require(
        required <= set(snapshot)
        and set(snapshot) <= required | {"engine", "failure"},
        f"{field} fields differ from the raw snapshot contract",
    )
    require(snapshot["schema_version"] == 1, f"{field} schema is unsupported")
    require_nonnegative_int(snapshot["event_sequence"], f"{field}.event_sequence")
    require(
        snapshot["lifecycle"] in SERVER_LIFECYCLES,
        f"{field} lifecycle is invalid",
    )
    _validate_qualification_clock(snapshot["clock"], f"{field}.clock", observed=False)
    protocol = snapshot["protocol"]
    require(isinstance(protocol, dict), f"{field}.protocol is malformed")
    for contract_field, expected in contracts.items():
        require(
            protocol.get(contract_field) == expected,
            f"{field}.protocol.{contract_field} is stale",
        )
    _validate_snapshot_identity(snapshot, field=field, package=package)
    engine = snapshot.get("engine")
    if engine is not None:
        require(isinstance(engine, dict), f"{field}.engine is malformed")
        for identity_field in ("engine_owner_id", "native_worker_id"):
            require_nonempty_string(
                engine.get(identity_field),
                f"{field}.engine.{identity_field}",
            )
        require_positive_int(
            engine.get("load_generation"),
            f"{field}.engine.load_generation",
        )
        require_positive_int(
            engine.get("model_load_count"),
            f"{field}.engine.model_load_count",
        )
    return snapshot


def _qualification_controls(
    value: object,
    *,
    name: str,
    contracts: dict,
    package: dict,
    nonce_sha256: str,
) -> QualificationControlEvidence:
    require(
        isinstance(value, list),
        f"qualification artifact {name} control events are malformed",
    )
    previous_sequence = -1
    actions = []
    allowed_actions = {
        "crash_server",
        "stall_native",
        "release_native",
        "hold_class",
        "release_class",
        "force_incompatible",
        "clear_incompatible",
        "snapshot",
        "freeze_owner",
        "release_owner",
    }
    for index, event in enumerate(value):
        field = f"qualification artifact {name} control event {index}"
        require(isinstance(event, dict), f"{field} is malformed")
        required = {
            "schema_version",
            "sequence",
            "action",
            "status",
            "authenticated_nonce_sha256",
            "server_event_sequence",
            "clock",
        }
        require(
            required <= set(event) and set(event) <= required | {"snapshot", "details"},
            f"{field} fields are invalid",
        )
        require(
            event["schema_version"] == 1,
            f"qualification artifact {name} control event schema is unsupported",
        )
        sequence = require_nonnegative_int(event["sequence"], f"{field}.sequence")
        require(
            sequence > previous_sequence,
            f"qualification artifact {name} control event sequence is not increasing",
        )
        previous_sequence = sequence
        action = require_nonempty_string(event["action"], f"{field}.action")
        require(
            action in allowed_actions,
            f"qualification artifact {name} used unknown control {action}",
        )
        actions.append(action)
        _validate_qualification_control_details(
            event,
            field=field,
            contracts=contracts,
            package=package,
            nonce_sha256=nonce_sha256,
        )
    return QualificationControlEvidence(tuple(value), tuple(actions))


def _validate_qualification_control_details(
    event: dict,
    *,
    field: str,
    contracts: dict,
    package: dict,
    nonce_sha256: str,
) -> None:
    require(
        event["status"] in {"completed", "accepted"},
        f"{field} did not complete",
    )
    require(
        event["authenticated_nonce_sha256"] == nonce_sha256,
        f"{field} was not authenticated",
    )
    require_nonnegative_int(
        event["server_event_sequence"],
        f"{field}.server_event_sequence",
    )
    _validate_qualification_clock(event["clock"], f"{field}.clock", observed=True)
    if "snapshot" in event:
        _validated_qualification_snapshot(
            event["snapshot"],
            field=f"{field}.snapshot",
            contracts=contracts,
            package=package,
        )
    if "details" in event:
        require(
            isinstance(event["details"], dict)
            and all(
                isinstance(key, str) and isinstance(value, str)
                for key, value in event["details"].items()
            ),
            f"{field}.details is malformed",
        )


_PROCESS_OBSERVATION_FIELDS = {
    "phase",
    "observed_ns",
    "server_instance_id",
    "pid",
    "process_start_id",
    "executable_sha256",
    "executable_version",
    "endpoint_namespace_id",
    "lifetime_authority_id",
    "listener_id",
    "protocol_sha256",
    "constant_set_sha256",
    "measurement_protocol_sha256",
    "load_generation",
    "snapshot",
}


def _validate_present_process_observation(
    observation: dict,
    snapshot: dict,
    *,
    field: str,
    contracts: dict,
) -> None:
    for contract_field, expected in contracts.items():
        require(
            observation[contract_field] == expected,
            f"{field}.{contract_field} is stale",
        )
    require(
        observation["server_instance_id"] == snapshot["process"]["server_instance_id"]
        and observation["pid"] == snapshot["process"]["pid"]
        and observation["process_start_id"] == snapshot["process"]["process_start_id"]
        and observation["executable_sha256"]
        == snapshot["process"]["executable_sha256"]
        and observation["executable_version"]
        == snapshot["process"]["executable_version"]
        and observation["endpoint_namespace_id"]
        == snapshot["authority"]["endpoint_namespace_id"]
        and observation["lifetime_authority_id"]
        == snapshot["authority"]["lifetime_authority_id"]
        and observation["listener_id"] == snapshot["authority"]["listener_id"],
        f"{field} identity disagrees with its snapshot",
    )
    engine = snapshot.get("engine")
    require(
        observation["load_generation"]
        == (engine.get("load_generation") if isinstance(engine, dict) else None),
        f"{field} load generation disagrees with its snapshot",
    )


def _qualification_process_observations(
    value: object,
    *,
    name: str,
    orchestration: QualificationOrchestration,
    contracts: dict,
    package: dict,
) -> tuple[dict, ...]:
    require(
        isinstance(value, list),
        f"qualification artifact {name} process observations are malformed",
    )
    for index, observation in enumerate(value):
        field = f"qualification artifact {name} process observation {index}"
        require(isinstance(observation, dict), f"{field} is malformed")
        require_exact_keys(observation, _PROCESS_OBSERVATION_FIELDS, field)
        require_nonempty_string(observation["phase"], f"{field}.phase")
        observed_ns = require_nonnegative_int(
            observation["observed_ns"],
            f"{field}.observed_ns",
        )
        require(
            orchestration.started_ns <= observed_ns <= orchestration.finished_ns,
            f"{field} escaped its block",
        )
        snapshot = observation["snapshot"]
        if snapshot is None:
            require(
                all(
                    observation[item] is None
                    for item in _PROCESS_OBSERVATION_FIELDS
                    - {"phase", "observed_ns", "snapshot"}
                ),
                f"qualification artifact {name} absent observation retained an identity",
            )
            continue
        normalized_snapshot = _validated_qualification_snapshot(
            snapshot,
            field=f"{field}.snapshot",
            contracts=contracts,
            package=package,
        )
        _validate_present_process_observation(
            observation,
            normalized_snapshot,
            field=field,
            contracts=contracts,
        )
    return tuple(value)


def _qualification_transitions(
    value: object,
    *,
    name: str,
    orchestration: QualificationOrchestration,
) -> QualificationTransitionEvidence:
    require(
        isinstance(value, list),
        f"qualification artifact {name} observations are malformed",
    )
    by_kind: dict[str, list[dict]] = {}
    for index, observation in enumerate(value):
        field = f"qualification artifact {name} observation {index}"
        require(isinstance(observation, dict), f"{field} is malformed")
        require_exact_keys(
            observation,
            {"sequence", "kind", "observed_ns", "values"},
            field,
        )
        require(
            observation["sequence"] == index,
            f"qualification artifact {name} observation sequence is not contiguous",
        )
        kind = require_nonempty_string(observation["kind"], f"{field}.kind")
        observed_ns = require_nonnegative_int(
            observation["observed_ns"],
            f"{field}.observed_ns",
        )
        require(
            orchestration.started_ns <= observed_ns <= orchestration.finished_ns,
            f"{field} escaped its block",
        )
        require(
            isinstance(observation["values"], dict),
            f"{field}.values is malformed",
        )
        by_kind.setdefault(kind, []).append(observation)
    return QualificationTransitionEvidence(tuple(value), by_kind)


_REQUIRED_TRANSITIONS = {
    "client_death": {
        "dead_client_work_observed",
        "other_client_continued",
        "client_terminated",
        "dead_client_work_reclaimed",
        "post_reclaim_other_client_query",
    },
    "cold_race": {"two_independent_processes", "single_server_convergence"},
    "frozen_owner": {"bounded_owner_unresponsive", "owner_identity_stable"},
    "incompatible_owner": {
        "active_owner_rejected",
        "idle_owner_draining",
        "compatible_replacement",
    },
    "mixed_queue": {
        "queues_saturated",
        "query_selected_before_bulk_backlog",
        "typed_capacity_retry_observed",
        "per_class_fifo_observed",
        "global_fifo_across_projects",
        "query_preference_observed",
        "bulk_resumed",
    },
    "server_crash": {
        "inflight_request_observed",
        "server_replaced",
        "query_replayed",
    },
    "true_idle_respawn": {
        "anti_idle_work_observed",
        "owner_preserved_across_idle_boundary",
        "anti_idle_work_reclaimed",
        "true_idle_wait",
        "idle_surfaces_exercised",
        "owner_absent_after_true_idle",
        "server_respawned",
    },
    "worker_stall": {
        "stalled_request_observed",
        "watchdog_fail_stop_observed",
        "unrelated_process_survived",
        "post_stall_replacement",
    },
}

_REQUIRED_CONTROLS = {
    "client_death": Counter({"hold_class": 2, "release_class": 2}),
    "cold_race": Counter(),
    "frozen_owner": Counter({"freeze_owner": 1, "release_owner": 1}),
    "incompatible_owner": Counter(
        {"force_incompatible": 1, "clear_incompatible": 1}
    ),
    "mixed_queue": Counter({"hold_class": 2, "release_class": 2}),
    "server_crash": Counter({"hold_class": 1, "crash_server": 1}),
    "true_idle_respawn": Counter({"hold_class": 2, "release_class": 2}),
    "worker_stall": Counter({"stall_native": 1, "release_native": 1}),
}


def _verify_cold_race_evidence(
    *,
    name: str,
    process_observations: tuple[dict, ...],
    transitions: QualificationTransitionEvidence,
) -> None:
    require(
        any(
            observation["phase"] == "cold_race_no_owner"
            and observation["snapshot"] is None
            for observation in process_observations
        ),
        f"qualification artifact {name} did not prove owner absence before the race",
    )
    independent = transitions.by_kind["two_independent_processes"][0]["values"]
    require(
        independent.get("first_pid") != independent.get("second_pid")
        and independent.get("first_project_identity_sha256")
        != independent.get("second_project_identity_sha256")
        and independent.get("first_transport_peer_verified") is True
        and independent.get("second_transport_peer_verified") is True,
        f"qualification artifact {name} cold-race processes were not independent",
    )


def _verify_scenario_artifact_requirements(
    *,
    name: str,
    scenario_id: str,
    controls: QualificationControlEvidence,
    process_observations: tuple[dict, ...],
    transitions: QualificationTransitionEvidence,
) -> None:
    require(
        all(
            len(transitions.by_kind.get(kind, [])) == 1
            for kind in _REQUIRED_TRANSITIONS[scenario_id]
        ),
        f"qualification artifact {name} omitted or duplicated required raw transitions",
    )
    actual_controls = Counter(controls.actions)
    require(
        all(
            actual_controls[action] >= count
            for action, count in _REQUIRED_CONTROLS[scenario_id].items()
        ),
        f"qualification artifact {name} omitted required authenticated controls",
    )
    if scenario_id == "cold_race":
        _verify_cold_race_evidence(
            name=name,
            process_observations=process_observations,
            transitions=transitions,
        )


def _qualification_events(
    value: object,
    *,
    name: str,
    orchestration: QualificationOrchestration,
) -> tuple[dict, ...]:
    require(
        isinstance(value, list) and value,
        f"qualification artifact {name} has no correlated events",
    )
    for index, event in enumerate(value):
        field = f"qualification artifact {name} event {index}"
        require(isinstance(event, dict), f"{field} is malformed")
        require_exact_keys(
            event,
            {
                "sequence",
                "source",
                "action",
                "observed_ns",
                "correlation_id",
                "values",
            },
            field,
        )
        require(
            event["sequence"] == index,
            f"qualification artifact {name} event sequence is not contiguous",
        )
        require_nonempty_string(event["source"], f"{field}.source")
        require_nonempty_string(event["action"], f"{field}.action")
        observed_ns = require_nonnegative_int(event["observed_ns"], f"{field}.observed_ns")
        require(
            orchestration.started_ns <= observed_ns <= orchestration.finished_ns,
            f"{field} escaped its block",
        )
        require(
            event["correlation_id"] is None
            or (
                isinstance(event["correlation_id"], str)
                and bool(event["correlation_id"])
            ),
            f"{field}.correlation_id is malformed",
        )
        require(isinstance(event["values"], dict), f"{field}.values is malformed")
    return tuple(value)


def _verify_qualification_summary(
    *,
    scenario_id: str,
    evidence: QualificationArtifactEvidence,
) -> None:
    expected_counts = {
        "process_count": (
            evidence.summary.process_count,
            len(evidence.orchestration.invocations),
        ),
        "control_event_count": (
            evidence.summary.control_event_count,
            len(evidence.controls.events),
        ),
        "process_observation_count": (
            evidence.summary.process_observation_count,
            len(evidence.process_observations),
        ),
        "observation_count": (
            evidence.summary.observation_count,
            len(evidence.transitions.observations),
        ),
        "event_count": (evidence.summary.event_count, len(evidence.events)),
    }
    for field, (retained, expected) in expected_counts.items():
        require(
            retained == expected,
            f"qualification scenario {scenario_id} summary {field} is stale",
        )


def qualification_artifact(
    artifact_root: Path,
    summary: object,
    *,
    scenario_id: str,
    contracts: dict,
    package: dict,
    same_account: dict,
    materialization: dict,
    nonce_sha256: str,
    forbidden_values: list[str],
) -> tuple[dict, dict]:
    normalized_summary, document = _qualification_artifact_document(
        artifact_root,
        summary,
        scenario_id=scenario_id,
        contracts=contracts,
        forbidden_values=forbidden_values,
    )
    payload = document.payload
    orchestration = _qualification_orchestration(
        payload["orchestration"],
        name=document.name,
    )
    controls = _qualification_controls(
        payload["control_events"],
        name=document.name,
        contracts=contracts,
        package=package,
        nonce_sha256=nonce_sha256,
    )
    process_observations = _qualification_process_observations(
        payload["process_observations"],
        name=document.name,
        orchestration=orchestration,
        contracts=contracts,
        package=package,
    )
    transitions = _qualification_transitions(
        payload["observations"],
        name=document.name,
        orchestration=orchestration,
    )
    _verify_scenario_artifact_requirements(
        name=document.name,
        scenario_id=scenario_id,
        controls=controls,
        process_observations=process_observations,
        transitions=transitions,
    )
    evidence = QualificationArtifactEvidence(
        normalized_summary,
        document,
        orchestration,
        controls,
        process_observations,
        transitions,
        _qualification_events(
            payload["events"],
            name=document.name,
            orchestration=orchestration,
        ),
    )
    _verify_qualification_summary(
        scenario_id=scenario_id,
        evidence=evidence,
    )
    assertions = derive_scenario_assertions(
        scenario_id,
        observations_by_kind=evidence.transitions.by_kind,
        process_observations=list(evidence.process_observations),
        invocations=list(evidence.orchestration.invocations),
        control_actions=list(evidence.controls.actions),
        same_account=same_account,
        materialization=materialization,
    )
    return (
        {
            "name": document.name,
            "sha256": hashlib.sha256(document.payload_bytes).hexdigest(),
        },
        assertions,
    )


def require_candidate_matrix_installation_source(
    cell_id: str | None,
    installation_source: str,
) -> None:
    alias = CANDIDATE_QUALIFICATION_MATRIX_ALIASES.get(cell_id)
    if alias is not None:
        require(
            installation_source == alias["installation_source"],
            "candidate qualification matrix alias requires candidate-installed provenance",
        )
