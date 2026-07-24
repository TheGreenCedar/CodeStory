"""Qualification control, snapshot, and process-observation validation."""

from __future__ import annotations

from .contracts import (
    require_exact_keys,
    require_nonempty_string,
    require_nonnegative_int,
    require_positive_int,
)
from .foundation import SERVER_LIFECYCLES, require
from .qualification_artifact_types import (
    QualificationControlEvidence,
    QualificationOrchestration,
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
        scheduler.get("query_capacity") == 64 and scheduler.get("bulk_capacity") == 64,
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
        required <= set(snapshot) and set(snapshot) <= required | {"engine", "failure"},
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
        and observation["executable_sha256"] == snapshot["process"]["executable_sha256"]
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
