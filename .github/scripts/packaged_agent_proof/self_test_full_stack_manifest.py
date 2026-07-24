"""Hostile native manifest self-tests."""

from __future__ import annotations

import json

from .archive import load_native_manifest
from .contracts import write_json
from .foundation import NATIVE_MANIFEST_FILE, ProofFailure
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
