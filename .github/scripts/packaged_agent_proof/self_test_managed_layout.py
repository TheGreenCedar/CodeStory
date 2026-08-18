"""Flat managed-install layout staging and divergence self-tests."""

from __future__ import annotations

import json
import shutil
import tempfile
from pathlib import Path

from .contract_primitives import sha256, write_json
from .candidate_installation import _managed_manifest as _candidate_managed_manifest
from .foundation import ProofFailure, require
from .managed_layout import (
    MANAGED_MANIFEST_NAME,
    managed_archive_name,
    managed_binary_name,
    managed_launcher_path,
    managed_version_root,
    package_root_name,
    require_provisioner_package_root,
    stage_flat_managed_install,
    verify_flat_managed_layout,
)

_VERSION = "0.0.0"
_TARGET = "windows-x64"


def _archive_name() -> str:
    return managed_archive_name(_VERSION, _TARGET)


def _fixture_unpacked(root: Path, name: str) -> tuple[Path, Path]:
    unpacked = root / name
    package_root = unpacked / package_root_name(_archive_name())
    package_root.mkdir(parents=True)
    (package_root / managed_binary_name(_TARGET)).write_bytes(b"launcher-bytes")
    generation = package_root / "native-generations" / "generation-1"
    generation.mkdir(parents=True)
    (generation / "runtime-module.dll").write_bytes(b"runtime-bytes")
    return unpacked, package_root


def _managed_manifest(package_root: Path) -> dict:
    return {
        "path": managed_binary_name(_TARGET),
        "sha256": sha256(package_root / managed_binary_name(_TARGET)),
        "version": _VERSION,
        "build_source": "candidate_archive",
        "repo_ref": "1" * 40,
        "archive": _archive_name(),
        "archive_url": f"candidate-archive:{'d' * 64}",
        "archive_sha256": "d" * 64,
        "archive_bytes": 4096,
        "target": _TARGET,
        "stdio_initialize_verified": True,
        "provisioned_at": f"candidate-proof:{'1' * 40}",
    }


def _flat_staging_test(root: Path) -> None:
    unpacked, package_root = _fixture_unpacked(root, "flat-unpacked")
    resolved_root = require_provisioner_package_root(
        unpacked, _archive_name(), _TARGET
    )
    require(
        resolved_root == package_root,
        "package root resolution did not return the archive root",
    )
    plugin_data = root / "flat-data"
    launcher = stage_flat_managed_install(
        package_root,
        plugin_data,
        _VERSION,
        _TARGET,
        _managed_manifest(package_root),
    )
    require(
        launcher == managed_launcher_path(plugin_data, _VERSION, _TARGET),
        "staged launcher is not at the flat managed launcher path",
    )
    version_root = managed_version_root(plugin_data, _VERSION)
    require(
        launcher.parent == version_root,
        "staged launcher is not a direct child of the managed version root",
    )
    require(
        (
            version_root / "native-generations" / "generation-1" / "runtime-module.dll"
        ).is_file(),
        "staging did not copy the package root contents into the version root",
    )
    require(
        verify_flat_managed_layout(plugin_data, _VERSION, _TARGET) == launcher,
        "flat layout verification did not resolve the staged launcher",
    )


def _nested_staging_rejection_test(root: Path) -> None:
    unpacked, package_root = _fixture_unpacked(root, "nested-unpacked")
    plugin_data = root / "nested-data"
    version_root = managed_version_root(plugin_data, _VERSION)
    # Reproduce the shipped regression: the whole extraction directory lands in
    # the version root, pushing the launcher one archive-root segment deeper.
    shutil.copytree(unpacked, version_root)
    nested_manifest = _managed_manifest(package_root)
    nested_manifest["path"] = (
        f"{package_root.name}/{managed_binary_name(_TARGET)}"
    )
    write_json(version_root / MANAGED_MANIFEST_NAME, nested_manifest)
    try:
        verify_flat_managed_layout(plugin_data, _VERSION, _TARGET)
    except ProofFailure:
        pass
    else:
        raise ProofFailure("nested managed install staging was accepted")


def _package_root_hostiles_test(root: Path) -> None:
    flat = root / "rootless-unpacked"
    flat.mkdir()
    (flat / managed_binary_name(_TARGET)).write_bytes(b"launcher-bytes")
    hostile_extractions = [("archive without a package root", flat)]
    crowded, _package_root = _fixture_unpacked(root, "crowded-unpacked")
    (crowded / "second-root").mkdir()
    hostile_extractions.append(("archive with a second extraction root", crowded))
    misnamed = root / "misnamed-unpacked"
    misnamed_root = misnamed / "codestory-cli-v9.9.9-windows-x64"
    misnamed_root.mkdir(parents=True)
    (misnamed_root / managed_binary_name(_TARGET)).write_bytes(b"launcher-bytes")
    hostile_extractions.append(("archive with a misnamed package root", misnamed))
    reserved, reserved_root = _fixture_unpacked(root, "reserved-unpacked")
    (reserved_root / MANAGED_MANIFEST_NAME).write_text("{}", encoding="utf-8")
    hostile_extractions.append(("package root shipping manifest.json", reserved))
    for label, unpacked in hostile_extractions:
        try:
            require_provisioner_package_root(unpacked, _archive_name(), _TARGET)
        except ProofFailure:
            pass
        else:
            raise ProofFailure(f"{label} was accepted as a provisioner package root")


def _manifest_divergence_rejection_test(root: Path) -> None:
    unpacked, package_root = _fixture_unpacked(root, "divergence-unpacked")
    plugin_data = root / "divergence-data"
    valid_manifest = _managed_manifest(package_root)
    launcher = stage_flat_managed_install(
        package_root,
        plugin_data,
        _VERSION,
        _TARGET,
        valid_manifest,
    )
    version_root = managed_version_root(plugin_data, _VERSION)
    manifest_path = version_root / MANAGED_MANIFEST_NAME
    hostile_manifests = [
        ("launcher digest mismatch", {"sha256": "e" * 64}),
        ("nested launcher path", {"path": f"{package_root.name}/{launcher.name}"}),
        ("foreign archive name", {"archive": managed_archive_name("9.9.9", _TARGET)}),
        ("foreign target", {"target": "linux-x64"}),
        ("unverified stdio handshake", {"stdio_initialize_verified": False}),
        ("missing archive bytes", {"archive_bytes": None}),
        ("nonpositive archive bytes", {"archive_bytes": 0}),
    ]
    for label, overrides in hostile_manifests:
        write_json(manifest_path, {**json.loads(json.dumps(valid_manifest)), **overrides})
        try:
            verify_flat_managed_layout(plugin_data, _VERSION, _TARGET)
        except ProofFailure:
            pass
        else:
            raise ProofFailure(f"managed manifest with {label} was accepted")
    write_json(manifest_path, valid_manifest)
    launcher.unlink()
    try:
        verify_flat_managed_layout(plugin_data, _VERSION, _TARGET)
    except ProofFailure:
        pass
    else:
        raise ProofFailure("managed manifest without its staged launcher was accepted")


def _candidate_manifest_archive_size_test(root: Path) -> None:
    archive = root / _archive_name()
    archive.write_bytes(b"candidate-archive-bytes")
    manifest = _candidate_managed_manifest(
        archive,
        {
            "asset_target": _TARGET,
            "binary": {"sha256": "a" * 64},
            "source": {"commit": "1" * 40},
        },
        _VERSION,
    )
    require(
        manifest["archive_bytes"] == archive.stat().st_size,
        "candidate managed manifest did not bind the exact archive byte length",
    )


def run_managed_layout_self_tests() -> None:
    with tempfile.TemporaryDirectory(prefix="codestory-managed-layout-") as raw:
        root = Path(raw)
        _flat_staging_test(root)
        _nested_staging_rejection_test(root)
        _package_root_hostiles_test(root)
        _manifest_divergence_rejection_test(root)
        _candidate_manifest_archive_size_test(root)
