"""Native binary markers and runtime identity contracts."""

from __future__ import annotations

import hashlib
from pathlib import Path

from .foundation import (
    LOWER_TIER_NONCLAIMS,
    NATIVE_ENGINE_MARKER_PREFIX,
    NATIVE_ENGINE_MARKER_SUFFIX,
    SERVER_PROOF_MARKER_PREFIX,
    SERVER_PROOF_MARKER_SUFFIX,
    SERVER_PROOF_SCHEMA_VERSION,
    ProofFailure,
    require,
)
from .contracts import (
    normalized_backend,
    require_sha256,
)


def binary_markers(path: Path, marker_prefix: str, marker_suffix: str) -> list[str]:
    prefix = marker_prefix.encode("ascii")
    suffix = marker_suffix.encode("ascii")
    markers: set[bytes] = set()
    overlap = b""
    with path.open("rb") as handle:
        while chunk := handle.read(1024 * 1024):
            block = overlap + chunk
            offset = 0
            while True:
                start = block.find(prefix, offset)
                if start < 0:
                    break
                end = block.find(suffix, start)
                if end < 0:
                    break
                end += len(suffix)
                markers.add(block[start:end])
                offset = end
            overlap = block[-4096:]
    decoded = []
    for marker in sorted(markers):
        try:
            decoded.append(marker.decode("ascii"))
        except UnicodeDecodeError as exc:
            raise ProofFailure(f"packaged marker {marker_prefix!r} is not ASCII") from exc
    return decoded


def native_engine_markers(path: Path) -> list[str]:
    return binary_markers(path, NATIVE_ENGINE_MARKER_PREFIX, NATIVE_ENGINE_MARKER_SUFFIX)


def server_proof_markers(path: Path) -> list[str]:
    return binary_markers(path, SERVER_PROOF_MARKER_PREFIX, SERVER_PROOF_MARKER_SUFFIX)


def ordered_contract_digest(domain: str, values: list[str]) -> str:
    digest = hashlib.sha256()
    for value in [domain, *values]:
        encoded = value.encode("utf-8")
        digest.update(len(encoded).to_bytes(8, "little"))
        digest.update(encoded)
    return digest.hexdigest()


def embedding_contract_digest(model: dict, embedding: dict, tokenizer: dict) -> str:
    string_fields = [
        (model, "file_name", False),
        (model, "sha256", False),
        (embedding, "family", False),
        (embedding, "query_prefix", False),
        (embedding, "document_prefix", True),
        (embedding, "pooling", False),
        (embedding, "normalization", False),
        (embedding, "element_type", False),
        (tokenizer, "container", False),
        (tokenizer, "tokenizer_sha256", False),
        (tokenizer, "config_sha256", False),
    ]
    for owner, field, allow_empty in string_fields:
        value = owner.get(field)
        require(
            isinstance(value, str) and (allow_empty or bool(value)),
            f"native embedding contract field {field} is invalid",
        )
    for owner, field in (
        (model, "size_bytes"),
        (embedding, "dimension"),
        (embedding, "vector_schema_version"),
    ):
        require(
            type(owner.get(field)) is int and owner[field] > 0,
            f"native embedding contract field {field} is invalid",
        )
    return ordered_contract_digest(
        "codestory-native-embedding-contract-v1",
        [
            model["file_name"],
            str(model["size_bytes"]),
            model["sha256"],
            embedding["family"],
            str(embedding["dimension"]),
            embedding["query_prefix"],
            embedding["document_prefix"],
            embedding["pooling"],
            embedding["normalization"],
            embedding["element_type"],
            str(embedding["vector_schema_version"]),
            tokenizer["container"],
            tokenizer["tokenizer_sha256"],
            tokenizer["config_sha256"],
        ],
    )


def parse_native_build_identity(identity: str) -> dict[str, str]:
    parts = identity.split("|")
    require(parts[0] == "codestory-native-engine-v1", "native engine build schema is unsupported")
    require(parts[-1] == "end", "native engine build identity terminator is missing")
    fields: dict[str, str] = {}
    for part in parts[1:-1]:
        require("=" in part, f"malformed native engine build field: {part!r}")
        key, value = part.split("=", 1)
        require(bool(key) and bool(value), f"empty native engine build field: {part!r}")
        require(key not in fields, f"duplicate native engine build field: {key}")
        fields[key] = value
    required = {
        "target",
        "os",
        "arch",
        "linkage",
        "backend_loading",
        "backends",
        "llama_cpp_crate",
        "llama_cpp_commit",
        "model_sha256",
        "embedding_contract_sha256",
        "model_embedded",
        "producer",
    }
    missing = sorted(required - fields.keys())
    require(not missing, "native engine build identity is missing fields: " + ", ".join(missing))
    return fields


def parse_server_proof_identity(identity: str) -> dict[str, object]:
    parts = identity.split("|")
    require(
        parts[0] == SERVER_PROOF_MARKER_PREFIX.removesuffix("|"),
        "embedding server proof schema is unsupported",
    )
    require(parts[-1] == "end", "embedding server proof identity terminator is missing")
    raw: dict[str, str] = {}
    for part in parts[1:-1]:
        require("=" in part, f"malformed embedding server proof field: {part!r}")
        key, value = part.split("=", 1)
        require(bool(key) and bool(value), f"empty embedding server proof field: {part!r}")
        require(key not in raw, f"duplicate embedding server proof field: {key}")
        raw[key] = value
    required = {
        "bootstrap",
        "protocol_schema",
        "protocol_sha256",
        "constant_set_sha256",
        "measurement_protocol_sha256",
        "clock_policy",
        "query_capacity",
        "bulk_capacity",
        "idle_timeout_ms",
    }
    missing = sorted(required - raw.keys())
    require(not missing, "embedding server proof identity is missing fields: " + ", ".join(missing))
    require(set(raw) == required, "embedding server proof identity contains unknown fields")
    for field in ("protocol_sha256", "constant_set_sha256", "measurement_protocol_sha256"):
        require_sha256(raw[field], f"embedding server proof {field}")
    require(raw["clock_policy"] == "awake_monotonic", "embedding server proof clock policy is unsupported")
    numeric: dict[str, int] = {}
    for field in ("bootstrap", "protocol_schema", "query_capacity", "bulk_capacity", "idle_timeout_ms"):
        try:
            numeric[field] = int(raw[field])
        except ValueError as exc:
            raise ProofFailure(f"embedding server proof {field} is not an integer") from exc
        require(numeric[field] > 0, f"embedding server proof {field} must be positive")
    require(numeric["bootstrap"] == 1, "embedding server bootstrap version is unsupported")
    require(numeric["protocol_schema"] == 1, "embedding server protocol schema is unsupported")
    require(numeric["query_capacity"] == 64, "embedding server query capacity is not the accepted value")
    require(numeric["bulk_capacity"] == 64, "embedding server bulk capacity is not the accepted value")
    require(numeric["idle_timeout_ms"] == 60_000, "embedding server idle timeout is not the accepted value")
    return {
        "schema_version": SERVER_PROOF_SCHEMA_VERSION,
        "bootstrap_version": numeric["bootstrap"],
        "protocol_schema_version": numeric["protocol_schema"],
        "protocol_sha256": raw["protocol_sha256"],
        "constant_set_sha256": raw["constant_set_sha256"],
        "measurement_protocol_sha256": raw["measurement_protocol_sha256"],
        "clock_policy": raw["clock_policy"],
        "query_capacity": numeric["query_capacity"],
        "bulk_capacity": numeric["bulk_capacity"],
        "idle_timeout_ms": numeric["idle_timeout_ms"],
        "lower_tier_nonclaims": sorted(LOWER_TIER_NONCLAIMS),
    }
def verify_runtime_against_manifest(
    manifest: dict,
    runtime: dict,
    expected_policy: str | None,
) -> dict:
    identities = [
        runtime.get("identity"),
        runtime.get("second_host_identity"),
        runtime.get("rejoin_identity"),
    ]
    require(all(isinstance(identity, dict) for identity in identities), "runtime proof omitted engine identity")
    engine = manifest["engine"]
    model = manifest["model"]
    accelerator = manifest["accelerator"]
    compiled_backends = engine["compiled_backends"]
    observed_backend = ""
    for label, identity in zip(("first plugin host", "second plugin host", "rejoined plugin host"), identities):
        require(
            identity.get("embedding_ggml_build_identity") == engine["build_identity"],
            f"{label} loaded a different native engine build than the package manifest",
        )
        require(
            identity.get("embedding_model_sha256") == model["sha256"],
            f"{label} loaded a different embedding model than the package manifest",
        )
        current_backend = normalized_backend(identity.get("embedding_backend"))
        require(
            current_backend in compiled_backends,
            f"{label} selected backend {current_backend!r} outside the compiled package contract",
        )
        if not observed_backend:
            observed_backend = current_backend
        require(current_backend == observed_backend, "native backend changed across process restart")

    policy = str(identities[0].get("embedding_policy") or "")
    require(policy == expected_policy, "runtime policy does not match the requested proof lane")
    if policy == "accelerated":
        expected_backend = accelerator.get("expected_protected_backend")
        require(
            isinstance(expected_backend, str) and bool(expected_backend),
            "this package target has no protected accelerator execution claim",
        )
        require(
            observed_backend == expected_backend,
            "runtime accelerator backend does not match the protected package contract",
        )
        execution = "proven_by_live_runtime"
        non_claim_reason = None
    else:
        require(policy == "cpu_explicit", "runtime used neither protected acceleration nor explicit CPU")
        require(observed_backend == "cpu", "explicit CPU proof selected a non-CPU backend")
        execution = "explicit_cpu_execution"
        non_claim_reason = (
            accelerator.get("non_claim_reason")
            or "explicit_cpu_execution_does_not_prove_acceleration"
        )

    return {
        "build_identity": engine["build_identity"],
        "model_sha256": model["sha256"],
        "policy": policy,
        "backend": observed_backend,
        "execution": execution,
        "answer_quality_claim": False,
        "non_claim_reason": non_claim_reason,
    }
