"""Embedding engine identity and accelerator evidence validation."""

from __future__ import annotations

from .foundation import SOFTWARE_ADAPTERS, require
from .server_identity import find_value


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
        len(digest) == 64 and all(char in "0123456789abcdefABCDEF" for char in digest),
        "status lacks an exact model digest",
    )
    require(
        bool(fields["embedding_ggml_build_identity"]),
        "status lacks the linked ggml build identity",
    )
    require(
        bool(fields["embedding_backend"]), "status lacks the selected embedding backend"
    )
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
    require(
        bool(fields["embedding_engine_instance_id"]),
        "status lacks the process engine identity",
    )
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
        require(
            bool(fields["embedding_engine_load_error"]),
            "failed reload did not retain its load error",
        )
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
    require(
        isinstance(model_layers, int) and model_layers > 0,
        "status lacks model layer count",
    )
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
