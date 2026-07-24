"""Archive and contract fixtures for packaged proof self-tests."""

from __future__ import annotations

import hashlib
import json
import struct
import zipfile
from dataclasses import dataclass
from pathlib import Path

from .archive_io import (
    expected_archive_digest,
    find_cli,
    unpack_archive,
)
from .contract_primitives import (
    assert_retained_json_privacy,
    sha256,
    write_json,
)
from .foundation import (
    MEASUREMENT_PROTOCOL,
    NATIVE_MANIFEST_FILE,
    SERVER_CONSTANT_SET,
    SERVER_PROTOCOL,
    ProofFailure,
    require,
)
from .native_contract_identity import (
    embedding_contract_digest,
    parse_server_proof_identity,
)
from .native_manifest import load_native_manifest
from .self_test_full_stack_types import FullStackFixture


@dataclass(frozen=True)
class NativeContractFixture:
    binary_payload: bytes
    build_identity: str
    protocol_sha256: str
    constant_set_sha256: str
    measurement_protocol_sha256: str
    model: dict
    embedding: dict
    tokenizer: dict
    embedding_contract_sha256: str
    server_proof: dict


def _prepare_contract_files(root: Path) -> tuple[Path, Path, Path]:
    self_protocol = root / SERVER_PROTOCOL.name
    self_protocol.write_bytes(SERVER_PROTOCOL.read_bytes())
    self_constant_set = root / SERVER_CONSTANT_SET.name
    unfrozen_constant_set = json.loads(SERVER_CONSTANT_SET.read_text(encoding="utf-8"))
    unfrozen_constant_set["status"] = "unfrozen"
    unfrozen_constant_set["calibration_required_values"] = {
        field: None for field in unfrozen_constant_set["calibration_required_values"]
    }
    unfrozen_constant_set["qualification_thresholds"] = {
        field: None for field in unfrozen_constant_set["qualification_thresholds"]
    }
    unfrozen_constant_set["freeze_record"] = None
    write_json(self_constant_set, unfrozen_constant_set)
    self_measurement_protocol = root / MEASUREMENT_PROTOCOL.name
    self_measurement_protocol.write_bytes(MEASUREMENT_PROTOCOL.read_bytes())
    return self_protocol, self_constant_set, self_measurement_protocol


def _retained_privacy_tests(root: Path) -> None:
    retained_privacy_path = root / "retained-privacy.json"
    write_json(
        retained_privacy_path,
        {"account_id": "account:" + "a" * 64, "transcript_sha256": "b" * 64},
    )
    assert_retained_json_privacy(retained_privacy_path, [str(root), "private query"])
    write_json(retained_privacy_path, {"request_text": "private query"})
    try:
        assert_retained_json_privacy(
            retained_privacy_path, [str(root), "private query"]
        )
    except ProofFailure:
        pass
    else:
        raise ProofFailure("retained evidence private request text was accepted")


def _build_native_contract(
    protocol: Path,
    constant_set: Path,
    measurement_protocol: Path,
) -> NativeContractFixture:
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
    protocol_sha256 = sha256(protocol)
    constant_set_sha256 = sha256(constant_set)
    measurement_protocol_sha256 = sha256(measurement_protocol)
    server_proof_identity = (
        "codestory-embedding-server-proof-v1|bootstrap=1|protocol_schema=1|"
        f"protocol_sha256={protocol_sha256}|"
        f"constant_set_sha256={constant_set_sha256}|"
        f"measurement_protocol_sha256={measurement_protocol_sha256}|"
        "clock_policy=awake_monotonic|query_capacity=64|bulk_capacity=64|"
        "idle_timeout_ms=60000|end"
    )
    binary_payload = (
        bytes(binary_header)
        + b"\0"
        + build_identity.encode("ascii")
        + b"\0"
        + server_proof_identity.encode("ascii")
        + b"\0"
    )
    server_proof = parse_server_proof_identity(server_proof_identity)
    server_proof["constant_set_status"] = "unfrozen"
    return NativeContractFixture(
        binary_payload=binary_payload,
        build_identity=build_identity,
        protocol_sha256=protocol_sha256,
        constant_set_sha256=constant_set_sha256,
        measurement_protocol_sha256=measurement_protocol_sha256,
        model=model,
        embedding=embedding,
        tokenizer=tokenizer,
        embedding_contract_sha256=contract_sha256,
        server_proof=server_proof,
    )


def _native_manifest(contract: NativeContractFixture) -> dict:
    return {
        "schema_version": 3,
        "release_version": "0.0.0",
        "asset_target": "macos-arm64",
        "source": {
            "commit": "1" * 40,
            "tree": "2" * 40,
            "tracked_dirty": False,
        },
        "binary": {
            "name": "codestory-cli",
            "sha256": hashlib.sha256(contract.binary_payload).hexdigest(),
            "format": "mach-o",
            "arch": "aarch64",
            "needed": [],
        },
        "runtime_executable": {
            "name": "codestory-cli",
            "sha256": hashlib.sha256(contract.binary_payload).hexdigest(),
            "format": "mach-o",
            "arch": "aarch64",
            "needed": [],
            "generation_id": None,
        },
        "runtime_artifacts": [],
        "engine": {
            "build_contract_schema_version": 2,
            "build_identity": contract.build_identity,
            "target_triple": "aarch64-apple-darwin",
            "target_os": "macos",
            "target_arch": "aarch64",
            "linkage": "static",
            "backend_loading": "builtin",
            "compiled_backends": ["cpu", "metal"],
            "llama_cpp_crate_version": "test",
            "llama_cpp_source_commit": "test",
            "embedding_contract_sha256": contract.embedding_contract_sha256,
        },
        "model": contract.model,
        "embedding": contract.embedding,
        "tokenizer_config": contract.tokenizer,
        "accelerator": {
            "cpu_fallback": "explicit_only",
            "package_claim": "compiled_capability_only",
            "runtime_execution": "not_proven_by_package",
            "expected_protected_backend": "metal",
            "non_claim_reason": None,
        },
        "server_proof": contract.server_proof,
    }


def _archive_tests(
    root: Path,
    binary_payload: bytes,
    valid_manifest: dict,
) -> dict:
    payload = root / "artifact.zip"
    with zipfile.ZipFile(payload, "w") as handle:
        handle.writestr("codestory-cli", binary_payload)
        handle.writestr(
            NATIVE_MANIFEST_FILE,
            json.dumps(valid_manifest, indent=2, sort_keys=True) + "\n",
        )
    checksum = root / "SHA256SUMS.txt"
    checksum.write_text(f"{sha256(payload)}  {payload.name}\n", encoding="utf-8")
    require(
        expected_archive_digest(checksum, payload) == sha256(payload),
        "checksum parser failed",
    )
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
    return manifest


def build_full_stack_fixture(root: Path) -> FullStackFixture:
    protocol, constant_set, measurement_protocol = _prepare_contract_files(root)
    _retained_privacy_tests(root)
    native = _build_native_contract(protocol, constant_set, measurement_protocol)
    valid_manifest = _native_manifest(native)
    manifest = _archive_tests(root, native.binary_payload, valid_manifest)
    return FullStackFixture(
        root=root,
        measurement_protocol=measurement_protocol,
        binary_payload=native.binary_payload,
        build_identity=native.build_identity,
        protocol_sha256=native.protocol_sha256,
        constant_set_sha256=native.constant_set_sha256,
        measurement_protocol_sha256=native.measurement_protocol_sha256,
        manifest=manifest,
        valid_manifest=valid_manifest,
    )
