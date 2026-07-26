"""Flat managed-CLI install layout shared by staging and verification.

The plugin provisioner (provisionManagedCli in
plugins/codestory/scripts/codestory-mcp.cjs) publishes a release by copying
the archive's single package root directly into
<plugin data>/codestory-cli/<version>/ and writing manifest.json beside the
launcher, keeping the executable path one flat segment on every platform.
Staging and every checker derive that layout from this module so the proof
fails whenever either side drifts from what managed provisioning installs.
"""

from __future__ import annotations

import json
import shutil
from pathlib import Path

from .contract_primitives import sha256, write_json
from .foundation import HEX_SHA256, TARGET_CONTRACTS, ProofFailure, require

MANAGED_CLI_DIR_NAME = "codestory-cli"
MANAGED_MANIFEST_NAME = "manifest.json"
# The provisioner owns these names inside the version root, so a package that
# shipped them would collide with provisioner state instead of installing.
RESERVED_PACKAGE_PATHS = ("manifest.json", ".provisioning")
_ARCHIVE_SUFFIXES = (".zip", ".tar.gz")


def managed_binary_name(asset_target: str) -> str:
    require(
        asset_target in TARGET_CONTRACTS,
        f"unsupported managed install target: {asset_target}",
    )
    return TARGET_CONTRACTS[asset_target]["binary_name"]


def managed_archive_name(version: str, asset_target: str) -> str:
    require(
        asset_target in TARGET_CONTRACTS,
        f"unsupported managed install target: {asset_target}",
    )
    suffix = "zip" if asset_target.startswith("windows-") else "tar.gz"
    return f"codestory-cli-v{version}-{asset_target}.{suffix}"


def package_root_name(archive_name: str) -> str:
    for suffix in _ARCHIVE_SUFFIXES:
        if archive_name.endswith(suffix) and len(archive_name) > len(suffix):
            return archive_name[: -len(suffix)]
    raise ProofFailure(
        f"managed install archive name is not a release asset: {archive_name}"
    )


def managed_version_root(plugin_data: Path, version: str) -> Path:
    return plugin_data / MANAGED_CLI_DIR_NAME / version


def managed_launcher_path(plugin_data: Path, version: str, asset_target: str) -> Path:
    return managed_version_root(plugin_data, version) / managed_binary_name(
        asset_target
    )


def require_provisioner_package_root(
    unpacked: Path,
    archive_name: str,
    asset_target: str,
) -> Path:
    root_name = package_root_name(archive_name)
    entries = sorted(entry.name for entry in unpacked.iterdir())
    require(
        entries == [root_name],
        f"managed install archive must contain exactly one package root named {root_name}",
    )
    package_root = unpacked / root_name
    require(
        package_root.is_dir() and not package_root.is_symlink(),
        "managed install package root must be a direct directory",
    )
    for reserved in RESERVED_PACKAGE_PATHS:
        require(
            not (package_root / reserved).exists(),
            f"managed install package root must not ship provisioner-owned {reserved}",
        )
    launcher = package_root / managed_binary_name(asset_target)
    require(
        launcher.is_file() and not launcher.is_symlink(),
        "managed install package root does not contain the launcher at its top level",
    )
    return package_root


def stage_flat_managed_install(
    package_root: Path,
    plugin_data: Path,
    version: str,
    asset_target: str,
    managed_manifest: dict,
) -> Path:
    version_root = managed_version_root(plugin_data, version)
    shutil.copytree(package_root, version_root)
    write_json(version_root / MANAGED_MANIFEST_NAME, managed_manifest)
    return verify_flat_managed_layout(plugin_data, version, asset_target)


def _managed_manifest_contents(version_root: Path) -> dict:
    manifest_path = version_root / MANAGED_MANIFEST_NAME
    require(
        manifest_path.is_file(),
        f"managed install manifest is missing: {manifest_path}",
    )
    try:
        manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
    except json.JSONDecodeError as exc:
        raise ProofFailure(
            f"managed install manifest is not valid JSON: {exc}"
        ) from exc
    require(isinstance(manifest, dict), "managed install manifest is not an object")
    return manifest


def verify_flat_managed_layout(
    plugin_data: Path,
    version: str,
    asset_target: str,
) -> Path:
    version_root = managed_version_root(plugin_data, version)
    manifest = _managed_manifest_contents(version_root)
    archive_name = managed_archive_name(version, asset_target)
    require(
        manifest.get("version") == version
        and manifest.get("target") == asset_target
        and manifest.get("archive") == archive_name
        and manifest.get("stdio_initialize_verified") is True
        and HEX_SHA256.fullmatch(str(manifest.get("archive_sha256") or "")) is not None,
        "managed install manifest does not describe this staged version",
    )
    launcher_name = managed_binary_name(asset_target)
    require(
        manifest.get("path") == launcher_name,
        "managed install manifest must point at the flat launcher beside it",
    )
    launcher = version_root / launcher_name
    require(
        launcher.is_file() and not launcher.is_symlink(),
        f"managed install launcher is missing from the flat version root: {launcher}",
    )
    require(
        HEX_SHA256.fullmatch(str(manifest.get("sha256") or "")) is not None
        and sha256(launcher) == manifest["sha256"],
        "managed install manifest digest does not match the staged launcher",
    )
    require(
        not (version_root / package_root_name(archive_name)).exists(),
        "managed install version root still nests the extracted archive root",
    )
    return launcher
