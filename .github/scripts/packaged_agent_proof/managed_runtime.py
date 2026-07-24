"""Managed installed-runtime verification."""

from __future__ import annotations

from pathlib import Path

from .contracts import require_nonempty_string, sha256
from .foundation import require
from .installation_support import same_existing_path


def verify_managed_runtime_status(
    status: dict,
    *,
    plugin_root: Path,
    manifest: dict,
    archive_sha256: str,
) -> dict:
    plugin = status.get("plugin_runtime")
    require(
        isinstance(plugin, dict), "installed status omitted plugin_runtime provenance"
    )
    require(
        plugin.get("cli_source") == "managed",
        "installed proof did not use the managed runtime",
    )
    require(
        plugin.get("local_dev_override") is False,
        "installed proof used a local CLI override",
    )
    require(
        plugin.get("plugin_version") == manifest["release_version"],
        "installed plugin version does not match the package",
    )
    reported_root = plugin.get("plugin_root")
    require(
        isinstance(reported_root, str)
        and same_existing_path(Path(reported_root), plugin_root),
        "installed status names a different plugin root",
    )
    require(
        plugin.get("managed_binary_sha256") == manifest["binary"]["sha256"],
        "installed managed executable does not match the package",
    )
    require(
        plugin.get("archive_sha256") == archive_sha256,
        "installed managed runtime names a different release archive",
    )
    require(
        plugin.get("cli_version") == manifest["release_version"],
        "installed managed executable version does not match the package",
    )
    managed_binary_path = plugin.get("managed_binary_path")
    require(
        isinstance(managed_binary_path, str) and Path(managed_binary_path).is_file(),
        "installed status omitted the managed executable path",
    )
    require(
        sha256(Path(managed_binary_path)) == manifest["binary"]["sha256"],
        "installed managed executable path does not contain the packaged binary",
    )
    for field in ("build_source", "repo_ref", "provisioned_at"):
        require_nonempty_string(plugin.get(field), f"installed plugin_runtime.{field}")
    return {
        "cli_source": "managed",
        "plugin_version": plugin["plugin_version"],
        "managed_binary_sha256": plugin["managed_binary_sha256"],
        "archive_sha256": plugin["archive_sha256"],
        "build_source": plugin["build_source"],
        "repo_ref": plugin["repo_ref"],
        "provisioned_at": plugin["provisioned_at"],
    }
