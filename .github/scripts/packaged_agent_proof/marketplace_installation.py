"""Marketplace checkout and installed-plugin provenance."""

from __future__ import annotations

import json
import re
import subprocess
from pathlib import Path

import tomllib

from .contract_primitives import require_exact_keys
from .foundation import PINNED_CODEX_CLI_VERSION, REPOSITORY_ROOT, require
from .installation_support import directory_contract_sha256, same_existing_path

_MARKETPLACE_NAME = "TheGreenCedar"
_MARKETPLACE_REPOSITORY = "TheGreenCedar/AgentPluginMarketplace"
_MARKETPLACE_URL = f"https://github.com/{_MARKETPLACE_REPOSITORY}.git"
_PLUGIN_ID = f"codestory@{_MARKETPLACE_NAME}"


def _git_output(repository: Path, *arguments: str) -> str:
    completed = subprocess.run(
        ["git", "-C", str(repository), *arguments],
        text=True,
        capture_output=True,
        timeout=30,
    )
    require(
        completed.returncode == 0,
        f"Git identity probe failed: {completed.stderr.strip()}",
    )
    return completed.stdout.strip()


def _marketplace_source(source_sha: str) -> dict[str, str]:
    return {
        "source": "git-subdir",
        "url": "https://github.com/TheGreenCedar/CodeStory.git",
        "path": "plugins/codestory",
        "sha": source_sha,
    }


def _validate_attestation_paths(
    attestation: dict,
    installed_plugin_data: Path,
    plugin_root: Path,
    manifest: dict,
) -> tuple[Path, dict, dict]:
    require_exact_keys(
        attestation,
        {
            "schema_version",
            "installation_source",
            "installation",
            "plugin",
            "marketplace",
        },
        "marketplace install attestation",
    )
    installation = attestation["installation"]
    plugin = attestation["plugin"]
    marketplace = attestation["marketplace"]
    require_exact_keys(
        installation,
        {"codex_home", "plugin_root", "plugin_data"},
        "marketplace installation paths",
    )
    require_exact_keys(
        plugin,
        {"id", "version", "source_commit", "source_tree", "package_sha256"},
        "marketplace installed plugin",
    )
    codex_home = Path(installation["codex_home"]).resolve()
    expected_plugin_root = (
        codex_home
        / "plugins"
        / "cache"
        / _MARKETPLACE_NAME
        / "codestory"
        / manifest["release_version"]
    )
    require(
        attestation["schema_version"] == 2
        and attestation["installation_source"] == "codex_marketplace_install"
        and codex_home.is_dir()
        and same_existing_path(Path(installation["plugin_root"]), plugin_root)
        and same_existing_path(Path(installation["plugin_data"]), installed_plugin_data)
        and installed_plugin_data.resolve().is_relative_to(codex_home)
        and same_existing_path(plugin_root, expected_plugin_root),
        "marketplace attestation does not identify the exact isolated Codex cache",
    )
    return codex_home, plugin, marketplace


def _validate_marketplace_results(
    marketplace: dict,
    codex_home: Path,
    plugin_root: Path,
    manifest: dict,
) -> tuple[Path, str]:
    require_exact_keys(
        marketplace,
        {
            "repository",
            "revision",
            "provenance",
            "codex_cli_version",
            "add_result",
            "list_result",
            "plugin_add_result",
            "plugin_list_result",
        },
        "marketplace install producer",
    )
    revision = marketplace["revision"]
    marketplace_add = marketplace["add_result"]
    require(
        marketplace["repository"] == _MARKETPLACE_REPOSITORY
        and marketplace["codex_cli_version"] == f"codex-cli {PINNED_CODEX_CLI_VERSION}"
        and isinstance(revision, str)
        and re.fullmatch(r"[0-9a-f]{40}", revision) is not None
        and isinstance(marketplace_add, dict)
        and marketplace_add.get("marketplaceName") == _MARKETPLACE_NAME
        and marketplace_add.get("alreadyAdded") is False,
        "marketplace attestation has an invalid pinned Codex producer",
    )
    marketplace_root_raw = marketplace_add.get("installedRoot")
    require(
        isinstance(marketplace_root_raw, str),
        "Codex marketplace add result omitted installedRoot",
    )
    marketplace_root = Path(marketplace_root_raw).resolve()
    expected_root = codex_home / ".tmp" / "marketplaces" / _MARKETPLACE_NAME
    require(
        marketplace_root.is_dir()
        and marketplace_root.is_relative_to(codex_home)
        and same_existing_path(marketplace_root, expected_root),
        "Codex marketplace root is outside its isolated home",
    )
    _validate_marketplace_list(marketplace, marketplace_root)
    plugin_source_sha = _validate_plugin_results(marketplace, plugin_root, manifest)
    return marketplace_root, plugin_source_sha


def _validate_marketplace_list(marketplace: dict, marketplace_root: Path) -> None:
    provenance = marketplace["provenance"]
    require_exact_keys(provenance, {"add", "list"}, "marketplace provenance")
    for operation in ("add", "list"):
        require_exact_keys(
            provenance[operation],
            {"root", "revision"},
            f"marketplace {operation} provenance",
        )
        require(
            same_existing_path(Path(provenance[operation]["root"]), marketplace_root)
            and provenance[operation]["revision"] == marketplace["revision"],
            "Codex marketplace add/list provenance does not report the pinned revision",
        )
    require(
        marketplace["list_result"]
        == {
            "marketplaces": [
                {
                    "name": _MARKETPLACE_NAME,
                    "root": str(marketplace_root),
                    "marketplaceSource": {
                        "sourceType": "git",
                        "source": _MARKETPLACE_URL,
                    },
                }
            ]
        },
        "Codex marketplace list does not match the configured Git snapshot",
    )


def _validate_plugin_results(
    marketplace: dict,
    plugin_root: Path,
    manifest: dict,
) -> str:
    plugin_list = marketplace["plugin_list_result"]
    installed = plugin_list.get("installed") if isinstance(plugin_list, dict) else None
    source = installed[0].get("source") if isinstance(installed, list) and len(installed) == 1 else None
    source_sha = source.get("sha") if isinstance(source, dict) else None
    require(
        isinstance(source_sha, str)
        and re.fullmatch(r"[0-9a-f]{40}", source_sha) is not None
        and source == _marketplace_source(source_sha),
        "Codex plugin source is not pinned to one immutable CodeStory commit",
    )
    require(
        marketplace["plugin_add_result"]
        == {
            "pluginId": _PLUGIN_ID,
            "name": "codestory",
            "marketplaceName": _MARKETPLACE_NAME,
            "version": manifest["release_version"],
            "installedPath": str(plugin_root),
            "authPolicy": "ON_INSTALL",
        },
        "Codex plugin add result does not identify the installed release plugin",
    )
    require(
        marketplace["plugin_list_result"]
        == {
            "installed": [
                {
                    "pluginId": _PLUGIN_ID,
                    "name": "codestory",
                    "marketplaceName": _MARKETPLACE_NAME,
                    "version": manifest["release_version"],
                    "installed": True,
                    "enabled": True,
                    "source": _marketplace_source(source_sha),
                    "marketplaceSource": {
                        "sourceType": "git",
                        "source": _MARKETPLACE_URL,
                    },
                    "installPolicy": "AVAILABLE",
                    "authPolicy": "ON_INSTALL",
                }
            ],
            "available": [],
        },
        "Codex plugin list does not contain exactly the enabled installed plugin",
    )
    return source_sha


def _validate_marketplace_checkout(
    codex_home: Path,
    marketplace_root: Path,
    marketplace: dict,
    plugin_source_sha: str,
) -> str:
    config = tomllib.loads((codex_home / "config.toml").read_text(encoding="utf-8"))
    marketplace_config = config.get("marketplaces", {}).get(_MARKETPLACE_NAME)
    plugin_config = config.get("plugins", {}).get(_PLUGIN_ID)
    require(
        isinstance(marketplace_config, dict)
        and marketplace_config.get("source_type") == "git"
        and marketplace_config.get("source") == _MARKETPLACE_URL
        and marketplace_config.get("ref") == marketplace["revision"]
        and plugin_config == {"enabled": True},
        "isolated Codex config does not pin the immutable marketplace revision",
    )
    marketplace_commit = _git_output(marketplace_root, "rev-parse", "HEAD")
    require(
        marketplace_commit == marketplace["revision"]
        and _git_output(marketplace_root, "status", "--porcelain") == ""
        and _git_output(marketplace_root, "remote", "get-url", "origin")
        == _MARKETPLACE_URL,
        "Codex marketplace checkout has invalid or mutable Git identity",
    )
    catalog = json.loads(
        (marketplace_root / ".agents" / "plugins" / "marketplace.json").read_text(
            encoding="utf-8"
        )
    )
    matches = [
        plugin
        for plugin in catalog.get("plugins", [])
        if plugin.get("name") == "codestory"
    ]
    require(
        len(matches) == 1
        and matches[0].get("source") == _marketplace_source(plugin_source_sha),
        "Codex marketplace catalog does not resolve CodeStory through the release repository",
    )
    return marketplace_commit


def _validate_release_source(plugin: dict, plugin_root: Path, manifest: dict) -> str:
    package_sha256 = directory_contract_sha256(plugin_root)
    source_commit = plugin["source_commit"]
    require(
        plugin["id"] == "codestory"
        and plugin["version"] == manifest["release_version"]
        and plugin["source_tree"] == manifest["source"]["tree"]
        and plugin["package_sha256"] == package_sha256
        and isinstance(source_commit, str)
        and re.fullmatch(r"[0-9a-f]{40}", source_commit) is not None
        and _git_output(REPOSITORY_ROOT, "rev-parse", f"{source_commit}^{{commit}}")
        == source_commit
        and _git_output(REPOSITORY_ROOT, "rev-parse", f"{source_commit}^{{tree}}")
        == manifest["source"]["tree"]
        and _git_output(REPOSITORY_ROOT, "rev-parse", "HEAD")
        == manifest["source"]["commit"]
        and _git_output(REPOSITORY_ROOT, "rev-parse", "HEAD^{tree}")
        == manifest["source"]["tree"],
        "marketplace install is not bound to the exact packaged release source",
    )
    source_plugin_root = REPOSITORY_ROOT / "plugins" / "codestory"
    require(
        package_sha256 == directory_contract_sha256(source_plugin_root),
        "Codex-installed plugin bytes differ from the packaged release source tree",
    )
    return package_sha256


def marketplace_installed_plugin_identity(
    attestation: dict,
    installed_plugin_data: Path,
    plugin_root: Path,
    manifest: dict,
) -> dict:
    codex_home, plugin, marketplace = _validate_attestation_paths(
        attestation,
        installed_plugin_data,
        plugin_root,
        manifest,
    )
    marketplace_root, plugin_source_sha = _validate_marketplace_results(
        marketplace,
        codex_home,
        plugin_root,
        manifest,
    )
    marketplace_commit = _validate_marketplace_checkout(
        codex_home,
        marketplace_root,
        marketplace,
        plugin_source_sha,
    )
    require(
        plugin["source_commit"] == plugin_source_sha,
        "marketplace attestation source does not match the catalog pin",
    )
    package_sha256 = _validate_release_source(plugin, plugin_root, manifest)
    return {
        "schema_version": 2,
        "installation_source": "codex_marketplace_install",
        "codex_cli_version": PINNED_CODEX_CLI_VERSION,
        "marketplace_repository": _MARKETPLACE_REPOSITORY,
        "marketplace_commit": marketplace_commit,
        "plugin_id": "codestory",
        "plugin_version": manifest["release_version"],
        "plugin_source_commit": plugin["source_commit"],
        "plugin_source_tree": plugin["source_tree"],
        "plugin_package_sha256": package_sha256,
    }
