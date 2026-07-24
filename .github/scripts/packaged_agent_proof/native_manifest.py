"""Native package manifest loading and validation."""

from __future__ import annotations

import json
from dataclasses import dataclass
from pathlib import Path

from native_binary_contract import (
    NativeBinaryError,
    inspect_binary,
    inspect_runtime_layout,
    runtime_artifact_role,
)

from .contract_primitives import sha256
from .foundation import (
    NATIVE_ENGINE_MARKER_PREFIX,
    NATIVE_MANIFEST_FILE,
    TARGET_CONTRACTS,
    ProofFailure,
    require,
)
from .native_contract_identity import (
    embedding_contract_digest,
    native_engine_markers,
    parse_native_build_identity,
    parse_server_proof_identity,
    server_proof_markers,
)


@dataclass(frozen=True)
class ManifestParts:
    binary: dict
    runtime_executable: dict
    source: dict
    engine: dict
    model: dict
    embedding: dict
    tokenizer: dict
    accelerator: dict
    server_proof: dict
    runtime_artifacts: list[dict]


@dataclass(frozen=True)
class NativeInventory:
    cli_sha256: str
    launcher_identity: dict
    runtime_path: Path
    binary_identity: dict
    runtime_artifacts: list[dict]


def _manifest_document(root: Path, expected_version: str) -> dict:
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
    require(
        manifest.get("schema_version") == 3,
        "native engine manifest schema is unsupported",
    )
    require(
        manifest.get("release_version") == expected_version,
        "native engine manifest version does not match expected release",
    )
    return manifest


def _manifest_parts(manifest: dict) -> ManifestParts:
    binary = manifest.get("binary")
    source = manifest.get("source")
    require(isinstance(binary, dict), "native engine manifest has no binary descriptor")
    require(isinstance(source, dict), "native engine manifest has no source descriptor")
    for field in ("commit", "tree"):
        value = source.get(field)
        require(
            isinstance(value, str)
            and len(value) == 40
            and all(char in "0123456789abcdef" for char in value),
            f"native engine manifest source {field} is invalid",
        )
    require(
        source.get("tracked_dirty") is False,
        "native engine manifest was built from tracked changes",
    )
    fields = (
        ("engine", "engine descriptor", dict),
        ("model", "model descriptor", dict),
        ("embedding", "embedding descriptor", dict),
        ("tokenizer_config", "tokenizer descriptor", dict),
        ("accelerator", "accelerator descriptor", dict),
        ("server_proof", "server proof descriptor", dict),
        ("runtime_executable", "runtime executable descriptor", dict),
        ("runtime_artifacts", "runtime artifact set", list),
    )
    values = {}
    for field, label, expected_type in fields:
        value = manifest.get(field)
        require(
            isinstance(value, expected_type),
            f"native engine manifest has no {label}",
        )
        values[field] = value
    return ManifestParts(
        binary=binary,
        runtime_executable=values["runtime_executable"],
        source=source,
        engine=values["engine"],
        model=values["model"],
        embedding=values["embedding"],
        tokenizer=values["tokenizer_config"],
        accelerator=values["accelerator"],
        server_proof=values["server_proof"],
        runtime_artifacts=values["runtime_artifacts"],
    )


def _runtime_artifact_paths(
    directory: Path,
    descriptors: list[dict],
    target_os: str,
) -> list[Path]:
    paths = []
    for descriptor in descriptors:
        require(
            isinstance(descriptor, dict),
            "native runtime artifact descriptor is invalid",
        )
        name = descriptor.get("name")
        require(
            isinstance(name, str) and name == Path(name).name,
            "native runtime artifact name is not a basename",
        )
        path = directory / name
        require(path.is_file(), f"native runtime artifact is missing: {name}")
        paths.append(path)
    discovered = sorted(
        (
            path.name
            for path in directory.iterdir()
            if path.is_file()
            and runtime_artifact_role(path.name, target_os) is not None
        ),
        key=str.lower,
    )
    require(
        discovered == sorted((path.name for path in paths), key=str.lower),
        "archive native runtime artifacts do not match the manifest",
    )
    return paths


def _runtime_path(cli: Path, parts: ManifestParts) -> tuple[Path, Path]:
    generation_id = parts.runtime_executable.get("generation_id")
    if generation_id is None:
        return cli, cli.parent
    require(
        isinstance(generation_id, str)
        and len(generation_id) == 64
        and all(char in "0123456789abcdef" for char in generation_id),
        "native runtime generation identity is invalid",
    )
    pointer = cli.parent / "codestory-native-current-generation-v1.txt"
    require(pointer.is_file(), "native runtime generation pointer is missing")
    require(
        pointer.read_text(encoding="utf-8").strip() == generation_id,
        "native runtime generation pointer contradicts the manifest",
    )
    directory = cli.parent / "codestory-native-generations" / generation_id
    runtime = directory / str(parts.runtime_executable.get("name"))
    require(runtime.is_file(), "native runtime executable is missing")
    return runtime, directory


def _native_inventory(
    cli: Path,
    parts: ManifestParts,
    target_contract: dict,
) -> NativeInventory:
    require(
        cli.name == target_contract["binary_name"],
        "packaged executable name does not match asset target",
    )
    require(
        parts.binary.get("name") == cli.name,
        "native engine manifest names a different binary",
    )
    cli_sha256 = sha256(cli)
    require(
        parts.binary.get("sha256") == cli_sha256,
        "packaged binary digest does not match native manifest",
    )
    runtime, runtime_directory = _runtime_path(cli, parts)
    artifact_paths = _runtime_artifact_paths(
        runtime_directory,
        parts.runtime_artifacts,
        target_contract["target_os"],
    )
    try:
        binary_identity, inspected_artifacts = inspect_runtime_layout(
            runtime,
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
        {
            **descriptor,
            "sha256": sha256(runtime_directory / str(descriptor["name"])),
        }
        for descriptor in inspected_artifacts
    ]
    require(
        parts.runtime_artifacts == inspected_artifacts,
        "native runtime artifact evidence is stale",
    )
    try:
        launcher_identity = inspect_binary(cli)
    except (OSError, NativeBinaryError) as exc:
        raise ProofFailure(f"packaged launcher is invalid: {exc}") from exc
    return NativeInventory(
        cli_sha256, launcher_identity, runtime, binary_identity, inspected_artifacts
    )


def _verify_binary_descriptor(
    cli: Path,
    parts: ManifestParts,
    inventory: NativeInventory,
    target_contract: dict,
) -> None:
    identity = inventory.launcher_identity
    require(
        parts.binary
        == {
            "name": cli.name,
            "sha256": inventory.cli_sha256,
            "format": identity["format"],
            "arch": identity["arch"],
            "needed": identity["needed"],
        },
        "native manifest binary descriptor does not match the executable",
    )
    require(
        identity["format"] == target_contract["binary_format"],
        "packaged executable format does not match asset target",
    )
    require(
        identity["arch"] == target_contract["target_arch"],
        "packaged executable architecture does not match asset target",
    )
    runtime_identity = inventory.binary_identity
    require(
        parts.runtime_executable
        == {
            "name": inventory.runtime_path.name,
            "sha256": sha256(inventory.runtime_path),
            "format": runtime_identity["format"],
            "arch": runtime_identity["arch"],
            "needed": runtime_identity["needed"],
            "generation_id": parts.runtime_executable.get("generation_id"),
        },
        "native manifest runtime executable descriptor is stale",
    )


def _build_fields(runtime: Path, parts: ManifestParts) -> dict[str, str]:
    require(
        parts.engine.get("build_contract_schema_version") == 2,
        "native engine build contract is unsupported",
    )
    build_identity = parts.engine.get("build_identity")
    require(
        isinstance(build_identity, str)
        and build_identity.startswith(NATIVE_ENGINE_MARKER_PREFIX)
        and build_identity.endswith("|end"),
        "native engine build identity is malformed",
    )
    build_fields = parse_native_build_identity(build_identity)
    require(
        native_engine_markers(runtime) == [build_identity],
        "packaged executable native engine marker does not match manifest",
    )
    server_markers = server_proof_markers(runtime)
    require(
        len(server_markers) == 1,
        "packaged executable must contain exactly one embedding server proof marker",
    )
    marker_server_proof = parse_server_proof_identity(server_markers[0])
    require(
        set(parts.server_proof) == {*marker_server_proof, "constant_set_status"},
        "native manifest server proof fields do not match the binary marker schema",
    )
    for field, value in marker_server_proof.items():
        require(
            parts.server_proof.get(field) == value,
            f"packaged executable embedding server proof field {field} does not match manifest",
        )
    require(
        parts.server_proof.get("constant_set_status") in {"unfrozen", "frozen"},
        "native manifest server proof constant-set status is invalid",
    )
    return build_fields


def _verify_engine(
    parts: ManifestParts,
    build_fields: dict[str, str],
    target_contract: dict,
) -> None:
    engine = parts.engine
    require(
        engine.get("linkage") == target_contract["linkage"],
        "packaged native engine linkage is wrong",
    )
    require(
        build_fields["linkage"] == engine["linkage"],
        "native build linkage contradicts manifest",
    )
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
    require(
        compiled_backends[0] == "cpu",
        "native manifest does not make CPU capability explicit",
    )
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


def _verify_model(
    parts: ManifestParts,
    build_fields: dict[str, str],
    expected_version: str,
) -> None:
    model_digest = parts.model.get("sha256")
    require(
        isinstance(model_digest, str)
        and len(model_digest) == 64
        and all(char in "0123456789abcdefABCDEF" for char in model_digest),
        "native manifest lacks an exact model digest",
    )
    require(
        parts.model.get("embedded") is True,
        "native manifest does not prove an embedded model",
    )
    require(
        build_fields["model_embedded"] == "true",
        "native build marker says the model is absent",
    )
    require(
        build_fields["model_sha256"] == model_digest,
        "native build model digest contradicts manifest",
    )
    contract_sha256 = embedding_contract_digest(
        parts.model,
        parts.embedding,
        parts.tokenizer,
    )
    require(
        build_fields["embedding_contract_sha256"] == contract_sha256,
        "native build embedding contract contradicts manifest",
    )
    require(
        parts.engine.get("embedding_contract_sha256") == contract_sha256,
        "native engine embedding contract digest contradicts manifest",
    )
    producer = parts.model.get("producer")
    require(isinstance(producer, dict), "native manifest lacks model producer identity")
    require(
        build_fields["producer"] == f"{producer.get('name')}@{producer.get('version')}",
        "native build producer contradicts manifest",
    )
    require(
        producer.get("version") == expected_version,
        "native model producer version does not match expected release",
    )


def _verify_accelerator(parts: ManifestParts, target_contract: dict) -> None:
    accelerator = parts.accelerator
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
    require(
        accelerator.get("expected_protected_backend")
        == target_contract["expected_protected_backend"],
        "native manifest protected backend does not match asset target",
    )
    require(
        accelerator.get("non_claim_reason") == target_contract["non_claim_reason"],
        "native manifest accelerator non-claim does not match asset target",
    )


def load_native_manifest(root: Path, cli: Path, expected_version: str) -> dict:
    manifest = _manifest_document(root, expected_version)
    asset_target = manifest.get("asset_target")
    target_contract = TARGET_CONTRACTS.get(asset_target)
    require(
        target_contract is not None,
        f"native manifest has unsupported asset target: {asset_target}",
    )
    parts = _manifest_parts(manifest)
    inventory = _native_inventory(cli, parts, target_contract)
    _verify_binary_descriptor(cli, parts, inventory, target_contract)
    build_fields = _build_fields(inventory.runtime_path, parts)
    _verify_engine(parts, build_fields, target_contract)
    _verify_model(parts, build_fields, expected_version)
    _verify_accelerator(parts, target_contract)
    return manifest
