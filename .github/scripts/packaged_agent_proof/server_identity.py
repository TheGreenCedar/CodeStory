"""Embedding engine and server snapshot identity."""

from __future__ import annotations

from .contracts import (
    require_nonempty_string,
    require_nonnegative_int,
    require_positive_int,
    require_sha256,
)
from .foundation import (
    RETRY_CLASSES,
    SERVER_LIFECYCLES,
    SERVER_PROOF_SCHEMA_VERSION,
    SOFTWARE_ADAPTERS,
    require,
)


def find_value(value: object, key: str) -> object | None:
    if isinstance(value, dict):
        if key in value:
            return value[key]
        for child in value.values():
            found = find_value(child, key)
            if found is not None:
                return found
    elif isinstance(value, list):
        for child in value:
            found = find_value(child, key)
            if found is not None:
                return found
    return None


def _engine_fields(status: dict) -> dict:
    return {
        key: find_value(status, key)
        for key in (
            "embedding_model_sha256",
            "embedding_ggml_build_identity",
            "embedding_backend",
            "embedding_adapter",
            "embedding_policy",
            "embedding_engine_instance_id",
            "embedding_engine_residency",
            "embedding_engine_load_generation",
            "embedding_engine_load_error",
            "embedding_model_load_count",
            "embedding_smoke_ms",
            "embedding_initialization_ms",
            "embedding_materialized_path",
            "embedding_materialized_reused",
            "embedding_accelerator_execution_verified",
            "embedding_execution_devices",
            "embedding_execution_backends",
            "embedding_execution_observation_source",
            "embedding_encode_count",
            "embedding_execution_node_count",
            "embedding_resident_accelerator_tensor_count",
            "embedding_resident_accelerator_tensor_bytes",
            "embedding_model_layer_count",
            "embedding_offloaded_layer_count",
        )
    }


def _verify_engine_core(
    fields: dict,
    *,
    expected_policy: str | None,
    expected_backend: str | None,
    expected_load_count: int,
    expected_load_generation: int,
    expected_residency: str,
    expected_load_error: bool,
) -> None:
    digest = str(fields["embedding_model_sha256"] or "")
    require(
        len(digest) == 64
        and all(char in "0123456789abcdefABCDEF" for char in digest),
        "status lacks an exact model digest",
    )
    require(bool(fields["embedding_ggml_build_identity"]), "status lacks the linked ggml build identity")
    require(bool(fields["embedding_backend"]), "status lacks the selected embedding backend")
    adapter = str(fields["embedding_adapter"] or "")
    require(adapter, "status lacks the physical adapter identity")
    require(
        not any(token in adapter.lower() for token in SOFTWARE_ADAPTERS),
        f"software adapter is not allowed: {adapter}",
    )
    require(
        fields["embedding_policy"] in {"accelerated", "cpu_explicit"},
        "status lacks an explicit embedding policy",
    )
    require(bool(fields["embedding_engine_instance_id"]), "status lacks the process engine identity")
    require(
        fields["embedding_engine_residency"] == expected_residency,
        f"engine residency is {fields['embedding_engine_residency']!r}, "
        f"expected {expected_residency!r}",
    )
    require(
        fields["embedding_model_load_count"] == expected_load_count,
        f"engine load count is {fields['embedding_model_load_count']!r}, "
        f"expected {expected_load_count}",
    )
    require(
        fields["embedding_engine_load_generation"] == expected_load_generation,
        f"engine load generation is {fields['embedding_engine_load_generation']!r}, "
        f"expected {expected_load_generation}",
    )
    if expected_load_error:
        require(bool(fields["embedding_engine_load_error"]), "failed reload did not retain its load error")
    else:
        require(
            fields["embedding_engine_load_error"] is None,
            f"engine retained an unexpected load error: {fields['embedding_engine_load_error']}",
        )
    for field, label in (
        ("embedding_smoke_ms", "timed live embedding smoke"),
        ("embedding_initialization_ms", "initialization timing"),
    ):
        require(
            isinstance(fields[field], (int, float)) and fields[field] >= 0,
            f"status lacks {label}",
        )
    if expected_policy:
        require(
            fields["embedding_policy"] == expected_policy,
            f"embedding policy is {fields['embedding_policy']!r}, expected {expected_policy!r}",
        )
    if expected_backend:
        observed = str(fields["embedding_backend"] or "").lower()
        expected = expected_backend.lower()
        require(
            expected in observed or (expected == "metal" and observed == "mtl"),
            f"embedding backend is {fields['embedding_backend']!r}, expected {expected_backend!r}",
        )


def _verify_accelerated_engine(fields: dict) -> None:
    if fields["embedding_policy"] != "accelerated":
        return
    require(
        fields["embedding_accelerator_execution_verified"] is True,
        "accelerated policy lacks live accelerator execution proof",
    )
    require(
        fields["embedding_execution_observation_source"] == "ggml_eval_callback",
        "accelerator execution source is unknown or inferred",
    )
    for field, label in (
        ("embedding_execution_devices", "execution device"),
        ("embedding_execution_backends", "execution backend"),
    ):
        require(
            isinstance(fields[field], list) and bool(fields[field]),
            f"status lacks an observed {label}",
        )
    for field, label in (
        ("embedding_encode_count", "successful encode counter"),
        ("embedding_execution_node_count", "backend-observed execution nodes"),
        (
            "embedding_resident_accelerator_tensor_count",
            "backend-observed resident accelerator tensors",
        ),
        (
            "embedding_resident_accelerator_tensor_bytes",
            "backend-observed resident accelerator tensor bytes",
        ),
    ):
        require(
            isinstance(fields[field], int) and fields[field] > 0,
            f"status lacks an advancing {label}"
            if field == "embedding_encode_count"
            else f"status lacks {label}",
        )
    model_layers = fields["embedding_model_layer_count"]
    require(isinstance(model_layers, int) and model_layers > 0, "status lacks model layer count")
    require(
        fields["embedding_offloaded_layer_count"] == model_layers,
        "not every model layer was offloaded",
    )


def engine_identity(
    status: dict,
    expected_policy: str | None,
    expected_backend: str | None,
    *,
    expected_load_count: int = 1,
    expected_load_generation: int = 1,
    expected_residency: str = "resident",
    expected_load_error: bool = False,
) -> dict:
    fields = _engine_fields(status)
    _verify_engine_core(
        fields,
        expected_policy=expected_policy,
        expected_backend=expected_backend,
        expected_load_count=expected_load_count,
        expected_load_generation=expected_load_generation,
        expected_residency=expected_residency,
        expected_load_error=expected_load_error,
    )
    _verify_accelerated_engine(fields)
    return fields


def _snapshot_clock_and_protocol(snapshot: dict, manifest: dict) -> tuple[dict, dict]:
    clock = snapshot.get("clock")
    require(isinstance(clock, dict), "embedding_server snapshot omitted clock identity")
    require(clock.get("domain") == "awake_monotonic", "embedding_server clock is not awake-monotonic")
    require_nonempty_string(clock.get("api"), "embedding_server.clock.api")
    require_nonempty_string(clock.get("boot_id"), "embedding_server.clock.boot_id")
    require_positive_int(clock.get("resolution_ns"), "embedding_server.clock.resolution_ns")
    protocol = snapshot.get("protocol")
    require(isinstance(protocol, dict), "embedding_server snapshot omitted protocol identity")
    require(protocol.get("bootstrap_version") == 1, "embedding_server bootstrap version is unsupported")
    require(protocol.get("schema_version") == 1, "embedding_server protocol version is unsupported")
    for field in ("protocol_sha256", "constant_set_sha256", "measurement_protocol_sha256"):
        require_sha256(protocol.get(field), f"embedding_server.protocol.{field}")
    server_proof = manifest.get("server_proof")
    require(isinstance(server_proof, dict), "package manifest omitted server_proof")
    for field in (
        "bootstrap_version",
        "protocol_schema_version",
        "protocol_sha256",
        "constant_set_sha256",
        "measurement_protocol_sha256",
    ):
        runtime_field = "schema_version" if field == "protocol_schema_version" else field
        require(
            protocol.get(runtime_field) == server_proof.get(field),
            f"runtime embedding server {runtime_field} does not match the package manifest",
        )
    return clock, protocol


def _snapshot_authority_and_process(snapshot: dict, manifest: dict) -> tuple[dict, dict]:
    authority = snapshot.get("authority")
    require(isinstance(authority, dict), "embedding_server snapshot omitted authority identity")
    for field in ("endpoint_namespace_id", "lifetime_authority_id", "listener_id"):
        require_nonempty_string(authority.get(field), f"embedding_server.authority.{field}")
    require(authority.get("peer_verified") is True, "embedding_server peer identity is not verified")
    process = snapshot.get("process")
    require(isinstance(process, dict), "embedding_server snapshot omitted process identity")
    for field in ("server_instance_id", "process_start_id", "executable_version"):
        require_nonempty_string(process.get(field), f"embedding_server.process.{field}")
    require_positive_int(process.get("pid"), "embedding_server.process.pid")
    require_sha256(process.get("executable_sha256"), "embedding_server.process.executable_sha256")
    require(
        process["executable_sha256"] == manifest["binary"]["sha256"],
        "embedding server process executable does not match the package manifest",
    )
    require(
        process["executable_version"] == manifest["release_version"],
        "embedding server process version does not match the package manifest",
    )
    return authority, process


def _snapshot_scheduler(snapshot: dict, manifest: dict) -> dict:
    scheduler = snapshot.get("scheduler")
    require(isinstance(scheduler, dict), "embedding_server snapshot omitted scheduler state")
    server_proof = manifest["server_proof"]
    require(
        scheduler.get("query_capacity") == server_proof.get("query_capacity") == 64,
        "embedding_server query capacity is not the manifest-bound accepted value",
    )
    require(
        scheduler.get("bulk_capacity") == server_proof.get("bulk_capacity") == 64,
        "embedding_server bulk capacity is not the manifest-bound accepted value",
    )
    for field in (
        "query_depth",
        "bulk_depth",
        "connection_count",
        "active_request_count",
        "lease_count",
    ):
        require_nonnegative_int(scheduler.get(field), f"embedding_server.scheduler.{field}")
    require(scheduler["query_depth"] <= 64, "embedding_server query depth exceeds capacity")
    require(scheduler["bulk_depth"] <= 64, "embedding_server bulk depth exceeds capacity")
    active_request = scheduler.get("active_request")
    if active_request is not None:
        require(isinstance(active_request, dict), "embedding_server active request is malformed")
        for field in ("request_id", "scope_id", "class", "phase"):
            require_nonempty_string(
                active_request.get(field),
                f"embedding_server.scheduler.active_request.{field}",
            )
        require(active_request["class"] in {"query", "bulk"}, "active request class is invalid")
        require_nonnegative_int(
            active_request.get("elapsed_ms"),
            "embedding_server.scheduler.active_request.elapsed_ms",
        )
    return scheduler


def _snapshot_engine_and_failure(
    snapshot: dict,
    *,
    require_resident: bool,
) -> tuple[dict | None, dict | None]:
    engine = snapshot.get("engine")
    if require_resident:
        require(isinstance(engine, dict), "resident embedding_server snapshot omitted engine identity")
    if engine is not None:
        require(isinstance(engine, dict), "embedding_server engine identity is malformed")
        for field in ("engine_owner_id", "native_worker_id"):
            require_nonempty_string(engine.get(field), f"embedding_server.engine.{field}")
        require_positive_int(engine.get("load_generation"), "embedding_server.engine.load_generation")
        require_positive_int(engine.get("model_load_count"), "embedding_server.engine.model_load_count")
        require_nonnegative_int(
            engine.get("successful_encode_count"),
            "embedding_server.engine.successful_encode_count",
        )
    failure = snapshot.get("failure")
    if failure is not None:
        require(isinstance(failure, dict), "embedding_server failure state is malformed")
        require_nonempty_string(failure.get("code"), "embedding_server.failure.code")
        require(
            failure.get("retry_class") in RETRY_CLASSES,
            "embedding_server failure retry class is invalid",
        )
        require_nonnegative_int(
            failure.get("retry_after_ms"),
            "embedding_server.failure.retry_after_ms",
        )
        require_nonempty_string(
            failure.get("retry_condition"),
            "embedding_server.failure.retry_condition",
        )
    return engine, failure


def server_snapshot(status: dict, manifest: dict, *, require_resident: bool) -> dict:
    snapshot = find_value(status, "embedding_server")
    require(isinstance(snapshot, dict), "diagnostics omitted the embedding_server snapshot")
    require(
        snapshot.get("schema_version") == SERVER_PROOF_SCHEMA_VERSION,
        "embedding_server snapshot schema is unsupported",
    )
    event_sequence = require_nonnegative_int(
        snapshot.get("event_sequence"),
        "embedding_server.event_sequence",
    )
    lifecycle = snapshot.get("lifecycle")
    require(lifecycle in SERVER_LIFECYCLES, "embedding_server lifecycle is invalid")
    clock, protocol = _snapshot_clock_and_protocol(snapshot, manifest)
    authority, process = _snapshot_authority_and_process(snapshot, manifest)
    scheduler = _snapshot_scheduler(snapshot, manifest)
    engine, failure = _snapshot_engine_and_failure(
        snapshot,
        require_resident=require_resident,
    )
    private_tokens = str(snapshot).lower()
    for forbidden in ("project_path", "project_root", "repository_path", "request_text"):
        require(forbidden not in private_tokens, f"embedding_server diagnostics leaked {forbidden}")
    return {
        "schema_version": snapshot["schema_version"],
        "event_sequence": event_sequence,
        "lifecycle": lifecycle,
        "clock": clock,
        "protocol": protocol,
        "authority": authority,
        "process": process,
        "scheduler": scheduler,
        "engine": engine,
        "failure": failure,
    }


def shared_server_identity(first: dict, second: dict) -> dict:
    for group, fields in (
        ("authority", ("endpoint_namespace_id", "lifetime_authority_id", "listener_id")),
        ("process", ("server_instance_id", "pid", "process_start_id", "executable_sha256")),
        ("engine", ("engine_owner_id", "native_worker_id", "load_generation", "model_load_count")),
    ):
        left = first.get(group)
        right = second.get(group)
        require(isinstance(left, dict) and isinstance(right, dict), f"shared proof omitted {group}")
        for field in fields:
            require(
                left.get(field) == right.get(field),
                f"independent plugin hosts observed different {group}.{field}",
            )
    require(
        first["engine"]["model_load_count"] == 1,
        "cold two-host race produced more than one model load",
    )
    return {
        "endpoint_namespace_id": first["authority"]["endpoint_namespace_id"],
        "lifetime_authority_id": first["authority"]["lifetime_authority_id"],
        "listener_id": first["authority"]["listener_id"],
        "server_instance_id": first["process"]["server_instance_id"],
        "server_process_start_id": first["process"]["process_start_id"],
        "engine_owner_id": first["engine"]["engine_owner_id"],
        "native_worker_id": first["engine"]["native_worker_id"],
        "load_generation": first["engine"]["load_generation"],
        "model_load_count": first["engine"]["model_load_count"],
    }


def assert_public_status(status: dict) -> None:
    require(
        find_value(status, "retrieval_mode") == "full",
        "public status does not report full retrieval",
    )
    maintainer_only = (
        "sidecar",
        "full_repair",
        "embedding_model_sha256",
        "embedding_ggml_build_identity",
        "embedding_backend",
        "embedding_adapter",
        "embedding_policy",
        "embedding_engine_instance_id",
        "embedding_engine_residency",
        "embedding_engine_load_generation",
        "embedding_engine_load_error",
        "embedding_materialized_path",
        "embedding_detected_provider",
        "embedding_detected_gpu",
        "embedding_server",
        "server_instance_id",
        "lifetime_authority_id",
        "listener_id",
        "engine_owner_id",
        "native_worker_id",
        "constant_set_sha256",
        "measurement_protocol_sha256",
    )
    leaked = [key for key in maintainer_only if find_value(status, key) is not None]
    require(
        not leaked,
        "public status leaked maintainer-only retrieval fields: " + ", ".join(leaked),
    )
