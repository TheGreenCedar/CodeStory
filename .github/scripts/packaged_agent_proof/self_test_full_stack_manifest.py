"""Hostile native manifest self-tests."""

from __future__ import annotations

import json
from unittest.mock import patch

from . import native_manifest
from .contract_primitives import write_json
from .foundation import NATIVE_MANIFEST_FILE, ProofFailure, require
from .native_contract_identity import parse_server_proof_identity, server_proof_markers
from .native_manifest import load_native_manifest
from .self_test_full_stack_types import FullStackFixture


def run_hostile_manifest_self_tests(fixture: FullStackFixture) -> None:
    root = fixture.root
    binary_payload = fixture.binary_payload
    build_identity = fixture.build_identity
    valid_manifest = fixture.valid_manifest
    hostile_root = root / "hostile-manifest"
    hostile_root.mkdir()
    hostile_cli = hostile_root / "codestory-cli"
    hostile_cli.write_bytes(binary_payload)
    write_json(hostile_root / NATIVE_MANIFEST_FILE, valid_manifest)
    load_native_manifest(hostile_root, hostile_cli, "0.0.0")
    marker = server_proof_markers(hostile_cli)[0]
    proof = parse_server_proof_identity(marker)
    require(
        proof["schema_version"] == 1 and proof["protocol_schema_version"] == 2,
        "native marker schema and wire protocol schema were conflated",
    )
    for version in (1, 3):
        stale_marker = marker.replace(
            "|protocol_schema=2|", f"|protocol_schema={version}|"
        )
        try:
            parse_server_proof_identity(stale_marker)
        except ProofFailure as error:
            require(
                str(error) == "embedding server protocol schema is unsupported",
                f"native marker protocol schema rejection changed: {error}",
            )
        else:
            raise ProofFailure(f"native marker protocol schema {version} was accepted")
    for field in (
        "schema_version", "bootstrap_version", "protocol_schema_version",
        "query_capacity", "bulk_capacity", "idle_timeout_ms",
    ):
        value = valid_manifest["server_proof"][field]
        for wrong_type in (float(value), bool(value), str(value)):
            hostile = json.loads(json.dumps(valid_manifest))
            hostile["server_proof"][field] = wrong_type
            write_json(hostile_root / NATIVE_MANIFEST_FILE, hostile)
            try:
                load_native_manifest(hostile_root, hostile_cli, "0.0.0")
            except ProofFailure as error:
                require(
                    str(error) == f"packaged executable embedding server proof field {field} "
                    "does not match manifest",
                    f"native marker numeric type rejection changed: {error}",
                )
            else:
                raise ProofFailure(
                    f"native marker/manifest {field} type {type(wrong_type).__name__} was accepted"
                )
    for field in (
        "protocol_sha256", "constant_set_sha256", "measurement_protocol_sha256"
    ):
        stale_proof = json.loads(json.dumps(valid_manifest))
        stale_proof["server_proof"][field] = "f" * 64
        write_json(hostile_root / NATIVE_MANIFEST_FILE, stale_proof)
        try:
            load_native_manifest(hostile_root, hostile_cli, "0.0.0")
        except ProofFailure as error:
            require(
                str(error) == f"packaged executable embedding server proof field {field} "
                "does not match manifest",
                f"native marker contract digest rejection changed: {error}",
            )
        else:
            raise ProofFailure(f"native marker/manifest {field} mismatch was accepted")
    hostile_manifest = json.loads(json.dumps(valid_manifest))
    hostile_manifest["binary"]["sha256"] = "0" * 64
    write_json(hostile_root / NATIVE_MANIFEST_FILE, hostile_manifest)
    try:
        load_native_manifest(hostile_root, hostile_cli, "0.0.0")
    except ProofFailure:
        pass
    else:
        raise ProofFailure("binary/manifest digest mismatch was accepted")

    # No supported asset pairs the fixture's Mach-O format with a foreign
    # arch, so name one: the wrong target must be rejected by the architecture
    # verifier itself, not earlier for being an unknown target.
    x64_macos = dict(native_manifest.TARGET_CONTRACTS["macos-arm64"])
    x64_macos["target_arch"] = "x86_64"
    x64_macos["target_triple"] = "x86_64-apple-darwin"
    wrong_target = json.loads(json.dumps(valid_manifest))
    wrong_target["asset_target"] = "macos-x64"
    write_json(hostile_root / NATIVE_MANIFEST_FILE, wrong_target)
    try:
        with patch.dict(native_manifest.TARGET_CONTRACTS, {"macos-x64": x64_macos}):
            load_native_manifest(hostile_root, hostile_cli, "0.0.0")
    except ProofFailure as error:
        # Either architecture check counts: the runtime-binary inventory runs
        # before the launcher descriptor, and both reject the same mismatch.
        require(
            "architecture does not match" in str(error)
            and "target" in str(error),
            "the asset-target negative was rejected before any architecture"
            f" check: {error}",
        )
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
