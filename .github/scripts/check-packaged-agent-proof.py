#!/usr/bin/env python3
"""Verify that a packaged CodeStory executable owns retrieval end to end.

The proof is deliberately process-shaped: one packaged executable, an embedded
model, an in-process engine, and no network or helper process preparation.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import queue
import re
import shutil
import stat
import struct
import subprocess
import sys
import tarfile
import tempfile
import threading
import time
import zipfile
from pathlib import Path

from native_binary_contract import (
    NativeBinaryError,
    inspect_runtime_layout,
    runtime_artifact_role,
)


STATUS_URI = "codestory://status"
ENGINE_DIAGNOSTICS_URI = "codestory://diagnostics/retrieval-engine"
DEFAULT_QUERY = "RuntimeContext"
DEFAULT_QUESTION = "Explain how CodeStory prepares retrieval."
SOFTWARE_ADAPTERS = ("llvmpipe", "lavapipe", "warp", "software rasterizer", "swiftshader")
LEGACY_TOKENS = (
    "llama-server",
    "repair-worker",
    "port-allocations",
    "native-embedding",
    "retrieval-sidecars",
    "sidecars",
)
LEGACY_HELP_TOKENS = ("llama-server", "sidecar", "repair", "consent", "download")
NATIVE_MANIFEST_FILE = "codestory-native-manifest.json"
NATIVE_ENGINE_MARKER_PREFIX = "codestory-native-engine-v1|"
NATIVE_ENGINE_MARKER_SUFFIX = "|end"
TARGET_CONTRACTS = {
    "linux-x64": {
        "binary_name": "codestory-cli",
        "binary_format": "elf",
        "target_triple": "x86_64-unknown-linux-gnu",
        "target_os": "linux",
        "target_arch": "x86_64",
        "compiled_backends": ["cpu", "vulkan"],
        "linkage": "dynamic",
        "backend_loading": "runtime-modules",
        "expected_protected_backend": None,
        "non_claim_reason": "linux_gpu_execution_is_not_a_release_claim",
    },
    "linux-arm64": {
        "binary_name": "codestory-cli",
        "binary_format": "elf",
        "target_triple": "aarch64-unknown-linux-gnu",
        "target_os": "linux",
        "target_arch": "aarch64",
        "compiled_backends": ["cpu", "vulkan"],
        "linkage": "dynamic",
        "backend_loading": "runtime-modules",
        "expected_protected_backend": None,
        "non_claim_reason": "linux_gpu_execution_is_not_a_release_claim",
    },
    "windows-x64": {
        "binary_name": "codestory-cli.exe",
        "binary_format": "pe",
        "target_triple": "x86_64-pc-windows-msvc",
        "target_os": "windows",
        "target_arch": "x86_64",
        "compiled_backends": ["cpu", "vulkan"],
        "linkage": "dynamic",
        "backend_loading": "runtime-modules",
        "expected_protected_backend": "vulkan",
        "non_claim_reason": None,
    },
    "windows-arm64": {
        "binary_name": "codestory-cli.exe",
        "binary_format": "pe",
        "target_triple": "aarch64-pc-windows-msvc",
        "target_os": "windows",
        "target_arch": "aarch64",
        "compiled_backends": ["cpu", "vulkan"],
        "linkage": "dynamic",
        "backend_loading": "runtime-modules",
        "expected_protected_backend": None,
        "non_claim_reason": "windows_arm64_accelerator_execution_is_not_protected",
    },
    "macos-x64": {
        "binary_name": "codestory-cli",
        "binary_format": "mach-o",
        "target_triple": "x86_64-apple-darwin",
        "target_os": "macos",
        "target_arch": "x86_64",
        "compiled_backends": ["cpu", "metal"],
        "linkage": "static",
        "backend_loading": "builtin",
        "expected_protected_backend": None,
        "non_claim_reason": "macos_x64_accelerator_execution_is_not_protected",
    },
    "macos-arm64": {
        "binary_name": "codestory-cli",
        "binary_format": "mach-o",
        "target_triple": "aarch64-apple-darwin",
        "target_os": "macos",
        "target_arch": "aarch64",
        "compiled_backends": ["cpu", "metal"],
        "linkage": "static",
        "backend_loading": "builtin",
        "expected_protected_backend": "metal",
        "non_claim_reason": None,
    },
}


class ProofFailure(RuntimeError):
    pass


def require(condition: bool, message: str) -> None:
    if not condition:
        raise ProofFailure(message)


def write_json(path: Path, value: object) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(value, indent=2, sort_keys=True) + "\n", encoding="utf-8")


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def expected_archive_digest(checksum_file: Path, archive: Path) -> str:
    lines = checksum_file.read_text(encoding="utf-8").splitlines()
    records: dict[str, str] = {}
    for line in lines:
        parts = line.strip().split()
        if len(parts) >= 2 and len(parts[0]) == 64:
            records[parts[-1].lstrip("*")] = parts[0].lower()
        elif len(parts) == 1 and len(parts[0]) == 64:
            records[archive.name] = parts[0].lower()
    require(archive.name in records, f"checksum file does not name {archive.name}")
    return records[archive.name]


def safe_target(root: Path, name: str) -> Path:
    target = (root / name).resolve()
    require(target.is_relative_to(root.resolve()), f"archive member escapes extraction root: {name}")
    return target


def unpack_archive(archive: Path, destination: Path) -> None:
    destination.mkdir(parents=True, exist_ok=True)
    if zipfile.is_zipfile(archive):
        with zipfile.ZipFile(archive) as handle:
            for member in handle.infolist():
                safe_target(destination, member.filename)
            handle.extractall(destination)
        return
    if tarfile.is_tarfile(archive):
        with tarfile.open(archive) as handle:
            members = handle.getmembers()
            for member in members:
                safe_target(destination, member.name)
                require(not member.issym() and not member.islnk(), f"archive contains link: {member.name}")
            handle.extractall(destination, members=members)
        return
    raise ProofFailure(f"unsupported archive format: {archive}")


def find_cli(root: Path) -> Path:
    names = {"codestory-cli", "codestory-cli.exe"}
    matches = [path for path in root.rglob("*") if path.is_file() and path.name in names]
    require(len(matches) == 1, f"archive must contain exactly one native CodeStory executable; found {len(matches)}")
    cli = matches[0]
    cli.chmod(cli.stat().st_mode | stat.S_IXUSR)
    return cli


def native_engine_markers(path: Path) -> list[str]:
    prefix = NATIVE_ENGINE_MARKER_PREFIX.encode("ascii")
    suffix = NATIVE_ENGINE_MARKER_SUFFIX.encode("ascii")
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
            raise ProofFailure("packaged native engine marker is not ASCII") from exc
    return decoded


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


def load_native_manifest(root: Path, cli: Path, expected_version: str) -> dict:
    matches = [path for path in root.rglob(NATIVE_MANIFEST_FILE) if path.is_file()]
    require(
        len(matches) == 1,
        f"archive must contain exactly one native engine manifest; found {len(matches)}",
    )
    try:
        manifest = json.loads(matches[0].read_text(encoding="utf-8"))
    except json.JSONDecodeError as exc:
        raise ProofFailure(f"native engine manifest is not valid JSON: {exc}") from exc
    require(isinstance(manifest, dict), "native engine manifest is not an object")
    require(manifest.get("schema_version") == 2, "native engine manifest schema is unsupported")
    require(
        manifest.get("release_version") == expected_version,
        "native engine manifest version does not match expected release",
    )
    asset_target = manifest.get("asset_target")
    target_contract = TARGET_CONTRACTS.get(asset_target)
    require(target_contract is not None, f"native manifest has unsupported asset target: {asset_target}")

    binary = manifest.get("binary")
    engine = manifest.get("engine")
    model = manifest.get("model")
    embedding = manifest.get("embedding")
    tokenizer = manifest.get("tokenizer_config")
    accelerator = manifest.get("accelerator")
    runtime_artifacts = manifest.get("runtime_artifacts")
    require(isinstance(binary, dict), "native engine manifest has no binary descriptor")
    require(isinstance(engine, dict), "native engine manifest has no engine descriptor")
    require(isinstance(model, dict), "native engine manifest has no model descriptor")
    require(isinstance(embedding, dict), "native engine manifest has no embedding descriptor")
    require(isinstance(tokenizer, dict), "native engine manifest has no tokenizer descriptor")
    require(isinstance(accelerator, dict), "native engine manifest has no accelerator descriptor")
    require(isinstance(runtime_artifacts, list), "native engine manifest has no runtime artifact set")
    require(
        cli.name == target_contract["binary_name"],
        "packaged executable name does not match asset target",
    )
    require(binary.get("name") == cli.name, "native engine manifest names a different binary")
    cli_sha256 = sha256(cli)
    require(binary.get("sha256") == cli_sha256, "packaged binary digest does not match native manifest")
    artifact_paths: list[Path] = []
    for descriptor in runtime_artifacts:
        require(isinstance(descriptor, dict), "native runtime artifact descriptor is invalid")
        name = descriptor.get("name")
        require(
            isinstance(name, str) and name == Path(name).name,
            "native runtime artifact name is not a basename",
        )
        path = cli.parent / name
        require(path.is_file(), f"native runtime artifact is missing: {name}")
        artifact_paths.append(path)
    discovered = sorted(
        [
            path.name
            for path in cli.parent.iterdir()
            if path.is_file()
            and runtime_artifact_role(path.name, target_contract["target_os"]) is not None
        ],
        key=str.lower,
    )
    require(
        discovered == sorted([path.name for path in artifact_paths], key=str.lower),
        "archive native runtime artifacts do not match the manifest",
    )
    try:
        binary_identity, inspected_artifacts = inspect_runtime_layout(
            cli,
            artifact_paths,
            target_os=target_contract["target_os"],
            expected_format=target_contract["binary_format"],
            expected_arch=target_contract["target_arch"],
            linkage=target_contract["linkage"],
            backend_loading=target_contract["backend_loading"],
        )
    except (OSError, NativeBinaryError) as exc:
        raise ProofFailure(f"packaged native runtime layout is invalid: {exc}") from exc
    inspected_artifacts = [
        {**descriptor, "sha256": sha256(cli.parent / str(descriptor["name"]))}
        for descriptor in inspected_artifacts
    ]
    require(runtime_artifacts == inspected_artifacts, "native runtime artifact evidence is stale")
    require(binary == {
        "name": cli.name,
        "sha256": cli_sha256,
        "format": binary_identity["format"],
        "arch": binary_identity["arch"],
        "needed": binary_identity["needed"],
    }, "native manifest binary descriptor does not match the executable")
    require(
        binary_identity["format"] == target_contract["binary_format"],
        "packaged executable format does not match asset target",
    )
    require(
        binary_identity["arch"] == target_contract["target_arch"],
        "packaged executable architecture does not match asset target",
    )
    require(engine.get("build_contract_schema_version") == 2, "native engine build contract is unsupported")
    build_identity = engine.get("build_identity")
    require(
        isinstance(build_identity, str)
        and build_identity.startswith(NATIVE_ENGINE_MARKER_PREFIX)
        and build_identity.endswith("|end"),
        "native engine build identity is malformed",
    )
    build_fields = parse_native_build_identity(build_identity)
    binary_markers = native_engine_markers(cli)
    require(
        binary_markers == [build_identity],
        "packaged executable native engine marker does not match manifest",
    )
    require(engine.get("linkage") == target_contract["linkage"], "packaged native engine linkage is wrong")
    require(build_fields["linkage"] == engine["linkage"], "native build linkage contradicts manifest")
    require(
        engine.get("backend_loading") == target_contract["backend_loading"],
        "packaged native backend loading mode is wrong",
    )
    require(
        build_fields["backend_loading"] == engine["backend_loading"],
        "native backend loading mode contradicts manifest",
    )
    compiled_backends = engine.get("compiled_backends")
    require(
        isinstance(compiled_backends, list)
        and compiled_backends
        and all(isinstance(item, str) and item for item in compiled_backends),
        "native manifest has no compiled backend set",
    )
    require(compiled_backends[0] == "cpu", "native manifest does not make CPU capability explicit")
    require(
        build_fields["backends"].split(",") == compiled_backends,
        "native build backend set contradicts manifest",
    )
    for manifest_field, build_field in (
        ("target_triple", "target"),
        ("target_os", "os"),
        ("target_arch", "arch"),
        ("llama_cpp_crate_version", "llama_cpp_crate"),
        ("llama_cpp_source_commit", "llama_cpp_commit"),
    ):
        require(
            engine.get(manifest_field) == build_fields[build_field],
            f"native build {build_field} contradicts manifest",
        )
    for manifest_field, expected in (
        ("target_triple", target_contract["target_triple"]),
        ("target_os", target_contract["target_os"]),
        ("target_arch", target_contract["target_arch"]),
    ):
        require(
            engine.get(manifest_field) == expected,
            f"native manifest {manifest_field} does not match asset target",
        )
    require(
        compiled_backends == target_contract["compiled_backends"],
        "native manifest backend set does not match asset target",
    )
    model_digest = model.get("sha256")
    require(
        isinstance(model_digest, str)
        and len(model_digest) == 64
        and all(char in "0123456789abcdefABCDEF" for char in model_digest),
        "native manifest lacks an exact model digest",
    )
    require(model.get("embedded") is True, "native manifest does not prove an embedded model")
    require(build_fields["model_embedded"] == "true", "native build marker says the model is absent")
    require(build_fields["model_sha256"] == model_digest, "native build model digest contradicts manifest")
    contract_sha256 = embedding_contract_digest(model, embedding, tokenizer)
    require(
        build_fields["embedding_contract_sha256"] == contract_sha256,
        "native build embedding contract contradicts manifest",
    )
    require(
        engine.get("embedding_contract_sha256") == contract_sha256,
        "native engine embedding contract digest contradicts manifest",
    )
    producer = model.get("producer")
    require(isinstance(producer, dict), "native manifest lacks model producer identity")
    require(
        build_fields["producer"] == f"{producer.get('name')}@{producer.get('version')}",
        "native build producer contradicts manifest",
    )
    require(
        producer.get("version") == expected_version,
        "native model producer version does not match expected release",
    )
    require(
        accelerator.get("cpu_fallback") == "explicit_only",
        "native manifest permits implicit CPU fallback",
    )
    require(
        accelerator.get("package_claim") == "compiled_capability_only",
        "native manifest overstates package-time accelerator proof",
    )
    require(
        accelerator.get("runtime_execution") == "not_proven_by_package",
        "native manifest overstates package-time execution proof",
    )
    expected_backend = accelerator.get("expected_protected_backend")
    require(
        expected_backend == target_contract["expected_protected_backend"],
        "native manifest protected backend does not match asset target",
    )
    require(
        accelerator.get("non_claim_reason") == target_contract["non_claim_reason"],
        "native manifest accelerator non-claim does not match asset target",
    )
    return manifest


def normalized_backend(value: object) -> str:
    backend = str(value or "").strip().lower()
    if backend == "mtl":
        return "metal"
    if backend.startswith("vulkan"):
        return "vulkan"
    return backend


def verify_runtime_against_manifest(
    manifest: dict,
    runtime: dict,
    expected_policy: str | None,
) -> dict:
    identities = [runtime.get("identity"), runtime.get("restart_identity")]
    require(all(isinstance(identity, dict) for identity in identities), "runtime proof omitted engine identity")
    engine = manifest["engine"]
    model = manifest["model"]
    accelerator = manifest["accelerator"]
    compiled_backends = engine["compiled_backends"]
    observed_backend = ""
    for label, identity in zip(("first process", "restart"), identities):
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


def run(command: list[str], *, env: dict[str, str], cwd: Path, timeout: int) -> dict:
    started = time.perf_counter()
    completed = subprocess.run(command, cwd=cwd, env=env, text=True, capture_output=True, timeout=timeout)
    result = {
        "command": command,
        "exit_code": completed.returncode,
        "wall_ms": round((time.perf_counter() - started) * 1000, 3),
        "stdout": completed.stdout,
        "stderr": completed.stderr,
    }
    require(completed.returncode == 0, f"command failed ({completed.returncode}): {' '.join(command)}\n{completed.stderr[-2000:]}")
    return result


def json_command(command: list[str], *, env: dict[str, str], cwd: Path, timeout: int) -> tuple[dict, dict]:
    result = run(command, env=env, cwd=cwd, timeout=timeout)
    try:
        payload = json.loads(result["stdout"])
    except json.JSONDecodeError as exc:
        raise ProofFailure(f"command did not emit JSON: {' '.join(command)}: {exc}") from exc
    require(isinstance(payload, dict), f"command emitted non-object JSON: {' '.join(command)}")
    return result, payload


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
    fields = {
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
    digest = str(fields["embedding_model_sha256"] or "")
    require(len(digest) == 64 and all(char in "0123456789abcdefABCDEF" for char in digest), "status lacks an exact model digest")
    require(bool(fields["embedding_ggml_build_identity"]), "status lacks the linked ggml build identity")
    require(bool(fields["embedding_backend"]), "status lacks the selected embedding backend")
    adapter = str(fields["embedding_adapter"] or "")
    require(adapter, "status lacks the physical adapter identity")
    require(not any(token in adapter.lower() for token in SOFTWARE_ADAPTERS), f"software adapter is not allowed: {adapter}")
    require(fields["embedding_policy"] in {"accelerated", "cpu_explicit"}, "status lacks an explicit embedding policy")
    require(bool(fields["embedding_engine_instance_id"]), "status lacks the process engine identity")
    require(fields["embedding_engine_residency"] == expected_residency, f"engine residency is {fields['embedding_engine_residency']!r}, expected {expected_residency!r}")
    require(fields["embedding_model_load_count"] == expected_load_count, f"engine load count is {fields['embedding_model_load_count']!r}, expected {expected_load_count}")
    require(fields["embedding_engine_load_generation"] == expected_load_generation, f"engine load generation is {fields['embedding_engine_load_generation']!r}, expected {expected_load_generation}")
    if expected_load_error:
        require(bool(fields["embedding_engine_load_error"]), "failed reload did not retain its load error")
    else:
        require(fields["embedding_engine_load_error"] is None, f"engine retained an unexpected load error: {fields['embedding_engine_load_error']}")
    require(isinstance(fields["embedding_smoke_ms"], (int, float)) and fields["embedding_smoke_ms"] >= 0, "status lacks the timed live embedding smoke")
    require(isinstance(fields["embedding_initialization_ms"], (int, float)) and fields["embedding_initialization_ms"] >= 0, "status lacks initialization timing")
    if expected_policy:
        require(fields["embedding_policy"] == expected_policy, f"embedding policy is {fields['embedding_policy']!r}, expected {expected_policy!r}")
    if expected_backend:
        observed = str(fields["embedding_backend"] or "").lower()
        expected = expected_backend.lower()
        matches = expected in observed or (expected == "metal" and observed == "mtl")
        require(matches, f"embedding backend is {fields['embedding_backend']!r}, expected {expected_backend!r}")
    if fields["embedding_policy"] == "accelerated":
        require(fields["embedding_accelerator_execution_verified"] is True, "accelerated policy lacks live accelerator execution proof")
        require(
            fields["embedding_execution_observation_source"] == "ggml_eval_callback",
            "accelerator execution source is unknown or inferred",
        )
        require(
            isinstance(fields["embedding_execution_devices"], list)
            and bool(fields["embedding_execution_devices"]),
            "status lacks an observed execution device",
        )
        require(
            isinstance(fields["embedding_execution_backends"], list)
            and bool(fields["embedding_execution_backends"]),
            "status lacks an observed execution backend",
        )
        require(
            isinstance(fields["embedding_encode_count"], int)
            and fields["embedding_encode_count"] > 0,
            "status lacks an advancing successful encode counter",
        )
        require(
            isinstance(fields["embedding_execution_node_count"], int)
            and fields["embedding_execution_node_count"] > 0,
            "status lacks backend-observed execution nodes",
        )
        require(
            isinstance(fields["embedding_resident_accelerator_tensor_count"], int)
            and fields["embedding_resident_accelerator_tensor_count"] > 0,
            "status lacks backend-observed resident accelerator tensors",
        )
        require(
            isinstance(fields["embedding_resident_accelerator_tensor_bytes"], int)
            and fields["embedding_resident_accelerator_tensor_bytes"] > 0,
            "status lacks backend-observed resident accelerator tensor bytes",
        )
        model_layers = fields["embedding_model_layer_count"]
        offloaded_layers = fields["embedding_offloaded_layer_count"]
        require(isinstance(model_layers, int) and model_layers > 0, "status lacks model layer count")
        require(offloaded_layers == model_layers, "not every model layer was offloaded")
    return fields


def parse_byte_quantity(value: str) -> int:
    match = re.fullmatch(r"([0-9]+(?:\.[0-9]+)?)([KMG])?", value.strip())
    require(match is not None, f"invalid memory quantity: {value!r}")
    scale = {None: 1, "K": 1024, "M": 1024**2, "G": 1024**3}[match.group(2)]
    return round(float(match.group(1)) * scale)


def process_resident_memory(pid: int) -> tuple[int, str]:
    if os.name == "nt":
        command = [
            "powershell",
            "-NoProfile",
            "-Command",
            f"(Get-Process -Id {pid} -ErrorAction Stop).WorkingSet64",
        ]
        scale = 1
        metric = "windows_working_set"
    elif sys.platform == "darwin":
        completed = subprocess.run(
            ["vmmap", "-summary", str(pid)],
            text=True,
            capture_output=True,
            timeout=20,
        )
        require(completed.returncode == 0, f"could not read physical footprint for process {pid}: {completed.stderr.strip()}")
        match = re.search(r"^Physical footprint:\s+([^\s]+)", completed.stdout, re.MULTILINE)
        require(match is not None, f"vmmap omitted the physical footprint for process {pid}")
        return parse_byte_quantity(match.group(1)), "macos_physical_footprint"
    else:
        command = ["ps", "-o", "rss=", "-p", str(pid)]
        scale = 1024
        metric = "rss"
    completed = subprocess.run(command, text=True, capture_output=True, timeout=10)
    require(completed.returncode == 0, f"could not read RSS for process {pid}: {completed.stderr.strip()}")
    try:
        return int(completed.stdout.strip()) * scale, metric
    except ValueError as exc:
        raise ProofFailure(f"invalid RSS for process {pid}: {completed.stdout!r}") from exc


def engine_process_id(identity: dict) -> int:
    parts = str(identity.get("embedding_engine_instance_id") or "").split(":")
    require(len(parts) >= 3 and parts[0] == "inprocess" and parts[1].isdigit(), "invalid process engine identity")
    return int(parts[1])


def assert_public_status(status: dict) -> None:
    require(find_value(status, "retrieval_mode") == "full", "public status does not report full retrieval")
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
    )
    leaked = [key for key in maintainer_only if find_value(status, key) is not None]
    require(not leaked, "public status leaked maintainer-only retrieval fields: " + ", ".join(leaked))


def extract_resource(response: dict, uri: str) -> dict:
    require("error" not in response, f"status resource failed: {response.get('error')}")
    contents = response.get("result", {}).get("contents", [])
    for item in contents:
        if isinstance(item, dict) and item.get("uri") == uri:
            payload = json.loads(item.get("text", "{}"))
            require(isinstance(payload, dict), "status resource emitted non-object JSON")
            return payload
    raise ProofFailure(f"resource response did not contain {uri}")


class McpProcess:
    def __init__(self, command: list[str], *, env: dict[str, str], cwd: Path, timeout: int):
        self.timeout = timeout
        self.process = subprocess.Popen(command, cwd=cwd, env=env, text=True, stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
        self.lines: queue.Queue[str | None] = queue.Queue()
        self.stderr: list[str] = []
        assert self.process.stdout and self.process.stderr and self.process.stdin
        threading.Thread(target=self._reader, args=(self.process.stdout, self.lines), daemon=True).start()
        threading.Thread(target=self._stderr_reader, daemon=True).start()
        self.transcript: list[dict] = []
        self.tool_attempt_counts: dict[str, int] = {}

    @staticmethod
    def _reader(stream, output: queue.Queue[str | None]) -> None:
        for line in stream:
            output.put(line)
        output.put(None)

    def _stderr_reader(self) -> None:
        assert self.process.stderr
        self.stderr.extend(self.process.stderr.readlines())

    def send(self, request: dict) -> dict:
        assert self.process.stdin
        self.process.stdin.write(json.dumps(request) + "\n")
        self.process.stdin.flush()
        deadline = time.monotonic() + self.timeout
        while True:
            remaining = deadline - time.monotonic()
            require(remaining > 0, f"MCP request timed out: {request.get('id')}")
            try:
                line = self.lines.get(timeout=remaining)
            except queue.Empty as exc:
                raise ProofFailure(f"MCP request timed out: {request.get('id')}") from exc
            require(line is not None, f"MCP process closed: {''.join(self.stderr)[-2000:]}")
            response = json.loads(line)
            self.transcript.append({"request": request, "response": response})
            if response.get("id") == request.get("id"):
                return response

    def initialize(self) -> None:
        response = self.send({
            "jsonrpc": "2.0",
            "id": "initialize",
            "method": "initialize",
            "params": {"protocolVersion": "2024-11-05", "capabilities": {}, "clientInfo": {"name": "packaged-proof", "version": "1"}},
        })
        require("error" not in response, f"MCP initialize failed: {response.get('error')}")
        assert self.process.stdin
        self.process.stdin.write(json.dumps({"jsonrpc": "2.0", "method": "notifications/initialized"}) + "\n")
        self.process.stdin.flush()

    def status(self, project: Path, request_id: str) -> dict:
        return extract_resource(self.send({
            "jsonrpc": "2.0",
            "id": request_id,
            "method": "resources/read",
            "params": {"uri": STATUS_URI, "project": str(project)},
        }), STATUS_URI)

    def engine_diagnostics(self, project: Path, request_id: str) -> dict:
        return extract_resource(self.send({
            "jsonrpc": "2.0",
            "id": request_id,
            "method": "resources/read",
            "params": {"uri": ENGINE_DIAGNOSTICS_URI, "project": str(project)},
        }), ENGINE_DIAGNOSTICS_URI)

    def tool(self, name: str, arguments: dict, request_id: str) -> dict:
        response = self.send({"jsonrpc": "2.0", "id": request_id, "method": "tools/call", "params": {"name": name, "arguments": arguments}})
        require("error" not in response, f"MCP {name} failed: {response.get('error')}")
        return response

    def tool_until_ready(self, name: str, arguments: dict, request_id: str) -> tuple[dict, int]:
        deadline = time.monotonic() + self.timeout
        attempt = 0
        while True:
            attempt += 1
            self.tool_attempt_counts[request_id] = attempt
            response = self.tool(name, arguments, f"{request_id}-{attempt}")
            result = response.get("result")
            require(
                isinstance(result, dict),
                f"MCP {name} attempt {attempt} returned a non-object result: {result!r}",
            )
            state = result.get("structuredContent")
            require(
                isinstance(state, dict),
                f"MCP {name} attempt {attempt} returned non-object structuredContent: {result!r}",
            )
            is_error = result.get("isError")
            if "isError" not in result or is_error is False:
                return response, attempt
            require(
                is_error is True,
                f"MCP {name} attempt {attempt} returned invalid isError={is_error!r}: {result!r}",
            )
            retry_state = (state.get("code"), state.get("state"))
            require(
                retry_state
                in (
                    ("codestory_preparing", "preparing"),
                    ("codestory_updating", "updating"),
                ),
                f"MCP {name} attempt {attempt} returned a terminal or malformed error envelope: {state!r}",
            )
            require(
                state.get("retry_tool") == name,
                f"MCP {name} attempt {attempt} returned the wrong retry tool: {state!r}",
            )
            retry_after_ms = state.get("retry_after_ms")
            require(
                isinstance(retry_after_ms, int)
                and not isinstance(retry_after_ms, bool)
                and retry_after_ms >= 0,
                f"MCP {name} attempt {attempt} returned invalid retry_after_ms: {state!r}",
            )
            remaining = deadline - time.monotonic()
            require(
                remaining > 0,
                f"MCP {name} did not become ready after attempt {attempt}: {state!r}",
            )
            delay_ms = min(retry_after_ms, max(0, int(remaining * 1000)))
            time.sleep(delay_ms / 1000)

    def search_until_ready(self, arguments: dict, request_id: str) -> tuple[dict, int]:
        response, attempts = self.tool_until_ready("search", arguments, request_id)
        result = response["result"]
        state = result["structuredContent"]
        query = arguments.get("query")
        require(
            isinstance(query, str) and state.get("query") == query,
            f"MCP search returned a mismatched query: expected {query!r}, response={state!r}",
        )
        require(
            isinstance(state.get("hits"), list),
            f"MCP search returned non-array hits: {state!r}",
        )
        retrieval = state.get("retrieval")
        require(
            isinstance(retrieval, dict) and retrieval.get("state") == "ready",
            f"MCP search did not return the ready installed retrieval projection: {state!r}",
        )
        # The installed result is deliberately compact. Full retrieval remains
        # proven separately by public status and activation diagnostics.
        return response, attempts

    def close(self) -> None:
        if self.process.stdin:
            self.process.stdin.close()
        try:
            self.process.wait(timeout=5)
        except subprocess.TimeoutExpired:
            self.process.terminate()
            try:
                self.process.wait(timeout=5)
            except subprocess.TimeoutExpired:
                self.process.kill()
                self.process.wait(timeout=5)


def isolated_environment(root: Path, policy: str | None, offline: bool) -> dict[str, str]:
    env = dict(os.environ)
    home = root / "home"
    cache = root / "cache"
    data = root / "plugin-data"
    temp = root / "tmp"
    for path in (home, cache, data, temp):
        path.mkdir(parents=True, exist_ok=True)
    env.update({
        "HOME": str(home),
        "USERPROFILE": str(home),
        "CODESTORY_CACHE_ROOT": str(cache),
        "CODESTORY_PLUGIN_DATA": str(data),
        "TMPDIR": str(temp),
        "TEMP": str(temp),
        "TMP": str(temp),
        "CODESTORY_EMBED_ALLOW_CPU": "1" if policy == "cpu_explicit" else "0",
    })
    if offline:
        env.update({
            "HTTP_PROXY": "http://127.0.0.1:1",
            "HTTPS_PROXY": "http://127.0.0.1:1",
            "ALL_PROXY": "http://127.0.0.1:1",
            "NO_PROXY": "",
            "CODESTORY_PLUGIN_DISABLE_PROVISION": "1",
        })
    for key in list(env):
        if key.startswith("CODESTORY_EMBED_") and key != "CODESTORY_EMBED_ALLOW_CPU":
            del env[key]
    return env


def assert_no_legacy_state(cache_root: Path) -> None:
    offenders = []
    for path in cache_root.rglob("*"):
        lowered = path.name.lower()
        if any(token in lowered for token in LEGACY_TOKENS) or path.suffix.lower() == ".pid":
            offenders.append(str(path))
    require(not offenders, "legacy process state was created: " + ", ".join(offenders[:10]))


def create_second_repository(root: Path) -> Path:
    repo = root / "second-repository"
    repo.mkdir()
    (repo / "README.md").write_text("# Second repository\n\nA tiny warm-engine reuse fixture.\n", encoding="utf-8")
    (repo / "lib.rs").write_text("pub fn shared_engine_probe() -> &'static str { \"warm\" }\n", encoding="utf-8")
    return repo


def native_cli_pid(mcp: McpProcess, plugin_handoff: bool) -> int:
    if not plugin_handoff:
        return mcp.process.pid
    deadline = time.monotonic() + 10
    while time.monotonic() < deadline:
        if os.name == "nt":
            command = [
                "powershell",
                "-NoProfile",
                "-Command",
                f"Get-CimInstance Win32_Process -Filter \"ParentProcessId = {mcp.process.pid}\" | Select-Object -ExpandProperty ProcessId",
            ]
            completed = subprocess.run(command, text=True, capture_output=True, timeout=10)
            candidates = [line.strip() for line in completed.stdout.splitlines()]
        else:
            completed = subprocess.run(
                ["ps", "-axo", "pid=,ppid="],
                text=True,
                capture_output=True,
                timeout=10,
            )
            candidates = []
            for line in completed.stdout.splitlines():
                fields = line.split()
                if len(fields) == 2 and fields[1] == str(mcp.process.pid):
                    candidates.append(fields[0])
        numeric = [int(value) for value in candidates if value.isdigit()]
        if len(numeric) == 1:
            return numeric[0]
        time.sleep(0.1)
    raise ProofFailure("plugin launcher did not expose exactly one native CLI child")


def create_residency_projects(root: Path) -> list[tuple[Path, str]]:
    projects = []
    for name, symbol in (("idle fixture alpha", "AlphaResidency"), ("idle fixture ü", "BetaResidency")):
        project = root / name
        source = project / "src"
        source.mkdir(parents=True, exist_ok=True)
        (project / "Cargo.toml").write_text(
            f'[package]\nname = "{name.replace(" ", "-").replace("ü", "u")}"\nversion = "0.1.0"\nedition = "2024"\n',
            encoding="utf-8",
        )
        (source / "lib.rs").write_text(
            f"pub struct {symbol};\nimpl {symbol} {{ pub fn ready(&self) -> bool {{ true }} }}\n",
            encoding="utf-8",
        )
        projects.append((project, symbol))
    return projects


def prove_idle_residency(
    args: argparse.Namespace,
    command: list[str],
    env: dict[str, str],
    out_dir: Path,
) -> dict:
    projects = create_residency_projects(out_dir / "idle-fixtures")
    project, query = projects[0]
    additional_projects = projects[1:]
    mcp = McpProcess(command, env=env, cwd=project, timeout=args.timeout_secs)
    try:
        mcp.initialize()
        mcp.status(project, "idle-baseline-status")
        for index, (additional_project, _) in enumerate(additional_projects, start=1):
            mcp.status(additional_project, f"idle-baseline-additional-{index}")
        cli_pid = native_cli_pid(mcp, args.plugin_handoff)
        baseline_memory, memory_metric = process_resident_memory(cli_pid)

        mcp.tool_until_ready(
            "search",
            {"project": str(project), "query": query, "why": True},
            "idle-load-search",
        )
        first_identity = engine_identity(
            mcp.engine_diagnostics(project, "idle-load-diagnostics"),
            args.engine_policy,
            args.expected_backend,
        )
        require(engine_process_id(first_identity) == cli_pid, "diagnostics identified a different CLI process")
        require(first_identity["embedding_materialized_reused"] is True, "measured process did not reuse the materialized model")
        for index, (additional_project, query) in enumerate(additional_projects, start=1):
            mcp.tool_until_ready(
                "search",
                {"project": str(additional_project), "query": query, "why": True},
                f"idle-load-additional-{index}",
            )
            additional_identity = engine_identity(
                mcp.engine_diagnostics(additional_project, f"idle-load-additional-diagnostics-{index}"),
                args.engine_policy,
                args.expected_backend,
            )
            require(additional_identity["embedding_engine_instance_id"] == first_identity["embedding_engine_instance_id"], "measured repositories did not share one owner")
        loaded_memory, loaded_metric = process_resident_memory(cli_pid)
        require(loaded_metric == memory_metric, "process memory metric changed during the proof")

        time.sleep(30)
        warm_identity = engine_identity(
            mcp.engine_diagnostics(project, "idle-still-warm-diagnostics"),
            args.engine_policy,
            args.expected_backend,
        )
        require(warm_identity["embedding_engine_instance_id"] == first_identity["embedding_engine_instance_id"], "recently active engine changed owner")
        time.sleep(35)
        sleeping_identity = engine_identity(
            mcp.engine_diagnostics(project, "idle-sleeping-diagnostics"),
            args.engine_policy,
            args.expected_backend,
            expected_residency="sleeping",
        )
        sleeping_memory, sleeping_metric = process_resident_memory(cli_pid)
        require(sleeping_metric == memory_metric, "process memory metric changed during the proof")
        loaded_increment = max(0, loaded_memory - baseline_memory)
        idle_allowance = max(50 * 1024 * 1024, loaded_increment // 4)
        require(
            sleeping_memory <= baseline_memory + idle_allowance,
            "idle process memory did not return near its pre-engine baseline: "
            f"metric={memory_metric} baseline={baseline_memory} loaded={loaded_memory} "
            f"sleeping={sleeping_memory} allowance={idle_allowance}",
        )
        assert_public_status(mcp.status(project, "idle-sleeping-status"))

        materialized = Path(str(first_identity["embedding_materialized_path"]))
        backup = materialized.with_name(materialized.name + ".proof-backup")
        materialized.rename(backup)
        materialized.mkdir()
        try:
            failed_wake = mcp.tool(
                "search",
                {"project": str(project), "query": query, "why": True},
                "idle-failed-wake",
            )
            require(failed_wake.get("result", {}).get("isError") is True, "blocked model path did not fail the activating wake")
            failed_identity = engine_identity(
                mcp.engine_diagnostics(project, "idle-failed-wake-diagnostics"),
                args.engine_policy,
                args.expected_backend,
                expected_residency="sleeping",
                expected_load_error=True,
            )
        finally:
            materialized.rmdir()
            backup.rename(materialized)

        mcp.tool_until_ready(
            "search",
            {"project": str(project), "query": query, "why": True},
            "idle-wake-search",
        )
        wake_identity = engine_identity(
            mcp.engine_diagnostics(project, "idle-wake-diagnostics"),
            args.engine_policy,
            args.expected_backend,
            expected_load_count=2,
            expected_load_generation=2,
        )
        require(wake_identity["embedding_engine_instance_id"] == first_identity["embedding_engine_instance_id"], "idle wake replaced the engine owner")
        require(wake_identity["embedding_materialized_path"] == first_identity["embedding_materialized_path"], "idle wake selected a different model path")
        require(wake_identity["embedding_materialized_reused"] is True, "idle wake rewrote the content-addressed model")
        result = {
            "memory_metric": memory_metric,
            "baseline_memory_bytes": baseline_memory,
            "loaded_memory_bytes": loaded_memory,
            "sleeping_memory_bytes": sleeping_memory,
            "idle_allowance_bytes": idle_allowance,
            "warm_identity": warm_identity,
            "sleeping_identity": sleeping_identity,
            "failed_identity": failed_identity,
            "wake_identity": wake_identity,
            "wake_memory_bytes": process_resident_memory(cli_pid)[0],
        }
        transcript = mcp.transcript
    finally:
        mcp.close()
    write_json(out_dir / "idle-residency-mcp.json", transcript)
    write_json(out_dir / "idle-residency.json", result)
    return result


def prove_runtime(args: argparse.Namespace, cli: Path, env: dict[str, str], root: Path, out_dir: Path) -> dict:
    project = args.project.resolve()
    if args.additional_project:
        require(
            len(args.additional_project) == len(args.additional_query),
            "each --additional-project requires one --additional-query",
        )
        additional_projects = [
            (path.resolve(), query)
            for path, query in zip(args.additional_project, args.additional_query)
        ]
    else:
        additional_projects = [(create_second_repository(root), "shared_engine_probe")]
    embedded_models = Path(env["CODESTORY_CACHE_ROOT"]) / "embedded-models"
    require(not embedded_models.exists(), "isolated proof cache was not empty before first use")

    command = [str(cli), "serve", "--stdio", "--multi-project", "--refresh", "none"]
    if args.plugin_handoff:
        require(args.plugin_root is not None, "--plugin-handoff requires --plugin-root")
        launcher = args.plugin_root.resolve() / "scripts" / "codestory-mcp.cjs"
        require(launcher.is_file(), f"plugin launcher is missing: {launcher}")
        env["CODESTORY_CLI"] = str(cli)
        command = [shutil.which("node") or "node", str(launcher)]
    mcp = McpProcess(command, env=env, cwd=project, timeout=args.timeout_secs)
    try:
        mcp.initialize()
        cold_started = time.perf_counter()
        _, cold_attempts = mcp.tool_until_ready(
            "ground", {"project": str(project), "budget": "strict"}, "cold-ground"
        )
        cold_ground_wall_ms = round((time.perf_counter() - cold_started) * 1000, 3)
        cold_models = list(embedded_models.rglob("*.gguf"))
        require(len(cold_models) == 1, "first use did not materialize exactly one embedded model")
        cold_materialization = {
            "path": str(cold_models[0]),
            "sha256": sha256(cold_models[0]),
            "first_command_wall_ms": cold_ground_wall_ms,
            "ground_attempts": cold_attempts,
        }
        mcp.tool_until_ready(
            "search", {"project": str(project), "query": args.query, "why": True}, "search-one"
        )
        public_status = mcp.status(project, "status-one")
        assert_public_status(public_status)
        first = mcp.engine_diagnostics(project, "diagnostics-one")
        first_identity = engine_identity(first, args.engine_policy, args.expected_backend)
        for index, (additional_project, query) in enumerate(additional_projects, start=1):
            mcp.tool_until_ready(
                "search",
                {"project": str(additional_project), "query": query, "why": True},
                f"search-additional-{index}",
            )
            additional_status = mcp.engine_diagnostics(
                additional_project, f"diagnostics-additional-{index}"
            )
            additional_identity = engine_identity(
                additional_status, args.engine_policy, args.expected_backend
            )
            require(
                first_identity["embedding_engine_instance_id"]
                == additional_identity["embedding_engine_instance_id"],
                "repositories did not share one process engine",
            )
            require(
                additional_identity["embedding_model_load_count"] == 1,
                "an additional repository reloaded the model",
            )
        mcp.tool_until_ready(
            "packet", {"project": str(project), "question": args.question, "budget": "compact"}, "packet"
        )
        final_status = mcp.engine_diagnostics(project, "diagnostics-final")
        final_identity = engine_identity(
            final_status, args.engine_policy, args.expected_backend
        )
        require(
            isinstance(first_identity["embedding_encode_count"], int)
            and isinstance(final_identity["embedding_encode_count"], int)
            and final_identity["embedding_encode_count"]
            > first_identity["embedding_encode_count"],
            "successful encode counter did not advance across live retrieval requests",
        )
        require(
            final_identity["embedding_engine_instance_id"]
            == first_identity["embedding_engine_instance_id"],
            "live retrieval requests replaced the process engine",
        )
        transcript = mcp.transcript
    finally:
        mcp.close()
    write_json(out_dir / "multi-repository-mcp.json", transcript)
    identity = final_identity

    materialized = Path(str(identity["embedding_materialized_path"] or ""))
    require(materialized.is_file(), f"materialized model is missing: {materialized}")
    require(sha256(materialized) == identity["embedding_model_sha256"], "materialized model digest does not match the embedded model")
    require(cold_materialization["sha256"] == identity["embedding_model_sha256"], "first-use model digest does not match engine identity")
    before_mtime = materialized.stat().st_mtime_ns

    idle_residency = None
    if args.idle_residency_proof:
        idle_residency = prove_idle_residency(
            args,
            command,
            env,
            out_dir,
        )

    restart = McpProcess([str(cli), "serve", "--stdio", "--multi-project", "--refresh", "none"], env=env, cwd=project, timeout=args.timeout_secs)
    restart_search_attempts = 0
    try:
        restart.initialize()
        _, restart_search_attempts = restart.search_until_ready(
            {"project": str(project), "query": args.query, "why": True},
            "restart-search",
        )
        restart_status = restart.engine_diagnostics(project, "restart-diagnostics")
        restart_identity = engine_identity(restart_status, args.engine_policy, args.expected_backend)
    finally:
        try:
            restart.close()
        finally:
            restart_search_attempts = restart.tool_attempt_counts.get(
                "restart-search", restart_search_attempts
            )
            write_json(
                out_dir / "restart-mcp.json",
                {
                    "restart_search_attempts": restart_search_attempts,
                    "transcript": restart.transcript,
                },
            )
    require(Path(str(restart_identity["embedding_materialized_path"])).resolve() == materialized.resolve(), "restart used a different materialized model")
    require(restart_identity["embedding_materialized_reused"] is True, "restart did not report content-addressed model reuse")
    require(materialized.stat().st_mtime_ns == before_mtime, "restart rewrote the materialized model")
    assert_no_legacy_state(Path(env["CODESTORY_CACHE_ROOT"]))
    return {
        "cold_materialization": cold_materialization,
        "identity": identity,
        "idle_residency": idle_residency,
        "restart_identity": restart_identity,
        "restart_search_attempts": restart_search_attempts,
    }


def self_test() -> None:
    require(parse_byte_quantity("24.1M") == 25_270_682, "memory quantity parser failed")

    class ScriptedMcpProcess(McpProcess):
        def __init__(self, responses: list[dict]):
            self.timeout = 1
            self.responses = iter(responses)
            self.calls: list[tuple[str, dict, str]] = []
            self.tool_attempt_counts: dict[str, int] = {}

        def tool(self, name: str, arguments: dict, request_id: str) -> dict:
            self.calls.append((name, arguments, request_id))
            try:
                return next(self.responses)
            except StopIteration as exc:
                raise ProofFailure("scripted MCP response sequence was exhausted") from exc

    projection_fixture_path = (
        Path(__file__).resolve().parents[2]
        / "crates"
        / "codestory-cli"
        / "tests"
        / "fixtures"
        / "stdio_installed_host_search_retrieval.json"
    )
    projection_fixture = json.loads(projection_fixture_path.read_text(encoding="utf-8"))
    require(
        isinstance(projection_fixture, dict),
        f"installed search projection fixture is not an object: {projection_fixture!r}",
    )
    ready_retrieval = projection_fixture.get("projected")
    require(
        isinstance(ready_retrieval, dict),
        f"installed search projection fixture is missing projected retrieval: {projection_fixture!r}",
    )

    query = "scripted-search"
    preparing = {
        "result": {
            "isError": True,
            "structuredContent": {
                "code": "codestory_preparing",
                "state": "preparing",
                "retry_tool": "search",
                "retry_after_ms": 0,
            },
        }
    }
    ready = {
        "result": {
            "structuredContent": {
                "query": query,
                "hits": [],
                "retrieval": ready_retrieval,
            }
        }
    }
    scripted = ScriptedMcpProcess([preparing, ready])
    _, attempts = scripted.search_until_ready({"query": query}, "self-test-search")
    require(attempts == 2, "preparing search did not converge on its second attempt")
    require(
        scripted.tool_attempt_counts.get("self-test-search") == 2,
        "preparing search attempt count was not retained",
    )

    unavailable = ScriptedMcpProcess([
        {
            "result": {
                "isError": True,
                "structuredContent": {
                    "code": "codestory_unavailable",
                    "state": "unavailable",
                    "message": "hostile terminal response",
                },
            }
        }
    ])
    try:
        unavailable.search_until_ready({"query": query}, "self-test-unavailable")
    except ProofFailure as exc:
        require(
            "codestory_unavailable" in str(exc),
            f"terminal MCP failure omitted its diagnostics: {exc}",
        )
    else:
        raise ProofFailure("terminal MCP unavailable response was retried or accepted")
    require(len(unavailable.calls) == 1, "terminal MCP unavailable response was retried")

    hostile_search_results = [
        (
            "legacy mode=full",
            {"query": query, "hits": [], "retrieval": {"mode": "full"}},
            "ready installed retrieval projection",
        ),
        (
            "preparing retrieval projection",
            {"query": query, "hits": [], "retrieval": {"state": "preparing"}},
            "ready installed retrieval projection",
        ),
        (
            "missing retrieval projection",
            {"query": query, "hits": []},
            "ready installed retrieval projection",
        ),
        (
            "non-array hits",
            {"query": query, "hits": {}, "retrieval": ready_retrieval},
            "non-array hits",
        ),
    ]
    for label, structured_content, expected_diagnostic in hostile_search_results:
        hostile = ScriptedMcpProcess(
            [{"result": {"structuredContent": structured_content}}]
        )
        try:
            hostile.search_until_ready({"query": query}, f"self-test-{label}")
        except ProofFailure as exc:
            require(
                expected_diagnostic in str(exc),
                f"{label} failure omitted its diagnostics: {exc}",
            )
        else:
            raise ProofFailure(f"{label} search result was accepted")
        require(len(hostile.calls) == 1, f"{label} search result was retried")

    with tempfile.TemporaryDirectory() as raw:
        root = Path(raw)
        payload = root / "artifact.zip"
        binary_header = bytearray(64)
        binary_header[:4] = b"\xcf\xfa\xed\xfe"
        struct.pack_into("<I", binary_header, 4, 0x0100000C)
        model = {
            "file_name": "test.gguf",
            "size_bytes": 4,
            "sha256": "a" * 64,
            "embedded": True,
            "producer": {"name": "test", "version": "0.0.0"},
        }
        embedding = {
            "family": "inprocess:test",
            "dimension": 768,
            "query_prefix": "query: ",
            "document_prefix": "",
            "pooling": "cls",
            "normalization": "l2",
            "element_type": "f32_le",
            "vector_schema_version": 2,
        }
        tokenizer = {
            "container": "gguf",
            "tokenizer_sha256": "b" * 64,
            "config_sha256": "c" * 64,
        }
        contract_sha256 = embedding_contract_digest(model, embedding, tokenizer)
        build_identity = (
            "codestory-native-engine-v1|target=aarch64-apple-darwin|os=macos|"
            "arch=aarch64|linkage=static|backend_loading=builtin|"
            "backends=cpu,metal|llama_cpp_crate=test|"
            f"llama_cpp_commit=test|model_sha256={'a' * 64}|"
            f"embedding_contract_sha256={contract_sha256}|model_embedded=true|"
            "producer=test@0.0.0|end"
        )
        binary_payload = bytes(binary_header) + b"\0" + build_identity.encode("ascii") + b"\0"
        valid_manifest = {
            "schema_version": 2,
            "release_version": "0.0.0",
            "asset_target": "macos-arm64",
            "binary": {
                "name": "codestory-cli",
                "sha256": hashlib.sha256(binary_payload).hexdigest(),
                "format": "mach-o",
                "arch": "aarch64",
                "needed": [],
            },
            "runtime_artifacts": [],
            "engine": {
                "build_contract_schema_version": 2,
                "build_identity": build_identity,
                "target_triple": "aarch64-apple-darwin",
                "target_os": "macos",
                "target_arch": "aarch64",
                "linkage": "static",
                "backend_loading": "builtin",
                "compiled_backends": ["cpu", "metal"],
                "llama_cpp_crate_version": "test",
                "llama_cpp_source_commit": "test",
                "embedding_contract_sha256": contract_sha256,
            },
            "model": model,
            "embedding": embedding,
            "tokenizer_config": tokenizer,
            "accelerator": {
                "cpu_fallback": "explicit_only",
                "package_claim": "compiled_capability_only",
                "runtime_execution": "not_proven_by_package",
                "expected_protected_backend": "metal",
                "non_claim_reason": None,
            },
        }
        with zipfile.ZipFile(payload, "w") as handle:
            handle.writestr("codestory-cli", binary_payload)
            handle.writestr(
                NATIVE_MANIFEST_FILE,
                json.dumps(valid_manifest, indent=2, sort_keys=True) + "\n",
            )
        checksum = root / "SHA256SUMS.txt"
        checksum.write_text(f"{sha256(payload)}  {payload.name}\n", encoding="utf-8")
        require(expected_archive_digest(checksum, payload) == sha256(payload), "checksum parser failed")
        unpacked = root / "unpacked"
        unpack_archive(payload, unpacked)
        cli = find_cli(unpacked)
        require(cli.name == "codestory-cli", "CLI discovery failed")
        manifest = load_native_manifest(unpacked, cli, "0.0.0")
        require(manifest["engine"]["linkage"] == "static", "manifest validation failed")
        malicious = root / "malicious.zip"
        with zipfile.ZipFile(malicious, "w") as handle:
            handle.writestr("../outside", b"bad")
        try:
            unpack_archive(malicious, root / "bad")
        except ProofFailure:
            pass
        else:
            raise ProofFailure("archive traversal was accepted")
        valid = {
            "embedding_model_sha256": "a" * 64,
            "embedding_ggml_build_identity": manifest["engine"]["build_identity"],
            "embedding_backend": "Metal",
            "embedding_adapter": "Apple GPU",
            "embedding_policy": "accelerated",
            "embedding_engine_instance_id": "engine-1",
            "embedding_engine_residency": "resident",
            "embedding_engine_load_generation": 1,
            "embedding_model_load_count": 1,
            "embedding_smoke_ms": 1.0,
            "embedding_initialization_ms": 2.0,
            "embedding_accelerator_execution_verified": True,
            "embedding_execution_devices": ["Apple GPU"],
            "embedding_execution_backends": ["Metal"],
            "embedding_execution_observation_source": "ggml_eval_callback",
            "embedding_encode_count": 1,
            "embedding_execution_node_count": 1,
            "embedding_resident_accelerator_tensor_count": 1,
            "embedding_resident_accelerator_tensor_bytes": 1,
            "embedding_model_layer_count": 13,
            "embedding_offloaded_layer_count": 13,
        }
        engine_identity(valid, "accelerated", "Metal")
        runtime = {"identity": valid, "restart_identity": valid.copy()}
        evidence = verify_runtime_against_manifest(manifest, runtime, "accelerated")
        require(evidence["execution"] == "proven_by_live_runtime", "runtime contract proof failed")
        invalid = {**valid, "embedding_adapter": "llvmpipe"}
        try:
            engine_identity(invalid, "accelerated", "Metal")
        except ProofFailure:
            pass
        else:
            raise ProofFailure("software adapter was accepted")
        inferred = {**valid, "embedding_execution_observation_source": "inferred_from_request"}
        try:
            engine_identity(inferred, "accelerated", "Metal")
        except ProofFailure:
            pass
        else:
            raise ProofFailure("inferred accelerator execution was accepted")

        hostile_root = root / "hostile-manifest"
        hostile_root.mkdir()
        hostile_cli = hostile_root / "codestory-cli"
        hostile_cli.write_bytes(binary_payload)
        hostile_manifest = json.loads(json.dumps(valid_manifest))
        hostile_manifest["binary"]["sha256"] = "0" * 64
        write_json(hostile_root / NATIVE_MANIFEST_FILE, hostile_manifest)
        try:
            load_native_manifest(hostile_root, hostile_cli, "0.0.0")
        except ProofFailure:
            pass
        else:
            raise ProofFailure("binary/manifest digest mismatch was accepted")

        wrong_target = json.loads(json.dumps(valid_manifest))
        wrong_target["asset_target"] = "macos-x64"
        write_json(hostile_root / NATIVE_MANIFEST_FILE, wrong_target)
        try:
            load_native_manifest(hostile_root, hostile_cli, "0.0.0")
        except ProofFailure:
            pass
        else:
            raise ProofFailure("asset target/binary architecture mismatch was accepted")

        stale_contract = json.loads(json.dumps(valid_manifest))
        stale_contract["embedding"]["query_prefix"] = "changed query: "
        write_json(hostile_root / NATIVE_MANIFEST_FILE, stale_contract)
        try:
            load_native_manifest(hostile_root, hostile_cli, "0.0.0")
        except ProofFailure:
            pass
        else:
            raise ProofFailure("stale binary embedding contract was accepted")

        marker_mismatch = json.loads(json.dumps(valid_manifest))
        marker_mismatch["engine"]["build_identity"] = build_identity.replace(
            "|end", "|note=fabricated|end"
        )
        write_json(hostile_root / NATIVE_MANIFEST_FILE, marker_mismatch)
        try:
            load_native_manifest(hostile_root, hostile_cli, "0.0.0")
        except ProofFailure:
            pass
        else:
            raise ProofFailure("binary/manifest native marker mismatch was accepted")
    print("packaged in-process embedding proof self-test passed")


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser()
    parser.add_argument("--archive", type=Path)
    parser.add_argument("--checksum-file", type=Path)
    parser.add_argument("--expected-version")
    parser.add_argument("--project", type=Path)
    parser.add_argument("--plugin-root", type=Path)
    parser.add_argument("--out-dir", type=Path, default=Path("target/packaged-agent-proof"))
    parser.add_argument("--query", default=DEFAULT_QUERY)
    parser.add_argument("--question", default=DEFAULT_QUESTION)
    parser.add_argument("--additional-project", type=Path, action="append", default=[])
    parser.add_argument("--additional-query", action="append", default=[])
    parser.add_argument("--timeout-secs", type=int, default=900)
    parser.add_argument("--version-only", action="store_true")
    parser.add_argument("--plugin-handoff", action="store_true")
    parser.add_argument("--engine-policy", choices=("accelerated", "cpu_explicit"))
    parser.add_argument("--expected-backend")
    parser.add_argument("--offline", action="store_true")
    parser.add_argument("--idle-residency-proof", action="store_true")
    parser.add_argument("--self-test", action="store_true")
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    if args.self_test:
        self_test()
        return 0
    require(args.archive and args.checksum_file and args.expected_version, "archive, checksum, and expected version are required")
    args.archive = args.archive.resolve()
    args.checksum_file = args.checksum_file.resolve()
    args.out_dir = args.out_dir.resolve()
    args.out_dir.mkdir(parents=True, exist_ok=True)
    require(sha256(args.archive) == expected_archive_digest(args.checksum_file, args.archive), "archive checksum mismatch")
    with tempfile.TemporaryDirectory(prefix="codestory-packaged-proof-") as raw:
        root = Path(raw)
        unpack_archive(args.archive, root / "unpacked")
        cli = find_cli(root / "unpacked")
        manifest = load_native_manifest(root / "unpacked", cli, args.expected_version)
        env = isolated_environment(root, args.engine_policy, args.offline)
        version = run([str(cli), "--version"], env=env, cwd=root, timeout=args.timeout_secs)
        require(args.expected_version in version["stdout"], f"CLI version does not contain {args.expected_version}")
        help_result = run([str(cli), "--help"], env=env, cwd=root, timeout=args.timeout_secs)
        help_text = help_result["stdout"].lower()
        require(
            not any(token in help_text for token in LEGACY_HELP_TOKENS),
            "top-level help exposes deleted embedding lifecycle terminology",
        )
        summary: dict[str, object] = {
            "version": version,
            "help": help_result,
            "package_contract": {
                "manifest": manifest,
                "answer_quality_claim": False,
            },
        }
        if not args.version_only:
            require(args.project is not None, "--project is required for the runtime proof")
            require(args.engine_policy is not None, "--engine-policy is required for the runtime proof")
            runtime = prove_runtime(args, cli, env, root, args.out_dir)
            summary["runtime"] = runtime
            summary["package_contract"]["runtime_evidence"] = verify_runtime_against_manifest(
                manifest, runtime, args.engine_policy
            )
        write_json(args.out_dir / "summary.json", summary)
    print(f"packaged CodeStory proof passed: {args.out_dir / 'summary.json'}")
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except (ProofFailure, subprocess.TimeoutExpired, OSError, json.JSONDecodeError) as exc:
        print(f"packaged CodeStory proof failed: {exc}", file=sys.stderr)
        raise SystemExit(1)
