"""Marketplace checkout and installed-plugin provenance.

Three delivery states resolve a released plugin through a Codex marketplace. Published uses the
live checkout; deferred and restored use distinct identities over the same candidate-pinned local
fixture shape:

* ``codex_marketplace_install`` -- the live public catalog. The resolver clones
  ``TheGreenCedar/AgentPluginMarketplace`` over Git into the isolated Codex home at a pinned
  ``ref``, so the checkout is remote-backed and carries that URL as its ``origin``.
* ``codex_marketplace_deferred_fixture`` -- a catalog built by
  ``.github/scripts/build-marketplace-fixture.mjs`` and pinned to the exact published commit,
  used when catalog publication was deferred. The resolver reads it as a *local* source: the
  marketplace root IS the fixture directory, the config records ``source_type = "local"`` with
  no ``ref``, and the repository has no ``origin`` remote at all.

Those are observed facts, not guesses: running the real pinned Codex CLI against a real fixture
produces ``sourceType: "local"`` and a marketplace root outside the Codex home, which is why the
live shape cannot describe a deferred install and must not be relaxed to try.

Nothing about the *plugin* differs between the states. The pinned ``git-subdir`` source, the
plugin add/list identity, the installed bytes, and the binding to the packaged release source are
verified identically, because the deferred state is a statement about which catalog served the
install -- never a statement that less was proved.
"""

from __future__ import annotations

import json
import re
import subprocess
from dataclasses import dataclass
from pathlib import Path

import tomllib

from .contract_primitives import require_exact_keys
from .foundation import PINNED_CODEX_CLI_VERSION, REPOSITORY_ROOT, require
from .installation_support import directory_contract_sha256, same_existing_path

_MARKETPLACE_NAME = "TheGreenCedar"
_MARKETPLACE_REPOSITORY = "TheGreenCedar/AgentPluginMarketplace"
_MARKETPLACE_URL = f"https://github.com/{_MARKETPLACE_REPOSITORY}.git"
_PLUGIN_ID = f"codestory@{_MARKETPLACE_NAME}"

LIVE_INSTALLATION_SOURCE = "codex_marketplace_install"
DEFERRED_INSTALLATION_SOURCE = "codex_marketplace_deferred_fixture"
RESTORED_INSTALLATION_SOURCE = "codex_marketplace_restored_fixture"
# Mirrors DEFERRED_MARKETPLACE_REPOSITORY in
# .github/scripts/marketplace-delivery-identity.mjs. Deliberately not a filesystem path and
# deliberately not shaped like "owner/repo": it can never be confused with the live catalog.
_DEFERRED_MARKETPLACE_REPOSITORY = "local:candidate-pinned-marketplace-fixture"
_RESTORED_MARKETPLACE_REPOSITORY = "local:candidate-pinned-marketplace-restored-fixture"
_FIXTURE_MARKER_FILENAME = ".codestory-marketplace-fixture.json"
_FIXTURE_MARKER_PURPOSE = "codestory-candidate-pinned-marketplace-fixture"


@dataclass(frozen=True)
class _DeliveryState:
    """The parts of the accepted shape that differ between the two catalog states."""

    installation_source: str
    repository: str
    source_type: str
    #: ``True`` when the resolver clones the catalog into the isolated Codex home.
    checkout_inside_codex_home: bool


_LIVE = _DeliveryState(
    installation_source=LIVE_INSTALLATION_SOURCE,
    repository=_MARKETPLACE_REPOSITORY,
    source_type="git",
    checkout_inside_codex_home=True,
)
_DEFERRED = _DeliveryState(
    installation_source=DEFERRED_INSTALLATION_SOURCE,
    repository=_DEFERRED_MARKETPLACE_REPOSITORY,
    source_type="local",
    checkout_inside_codex_home=False,
)
_RESTORED = _DeliveryState(
    installation_source=RESTORED_INSTALLATION_SOURCE,
    repository=_RESTORED_MARKETPLACE_REPOSITORY,
    source_type="local",
    checkout_inside_codex_home=False,
)

_DELIVERY_STATES = {
    state.installation_source: state for state in (_LIVE, _DEFERRED, _RESTORED)
}


def delivery_state(installation_source: object) -> _DeliveryState | None:
    """The accepted shape for an installer identity, or ``None`` if it names no marketplace."""
    if not isinstance(installation_source, str):
        return None
    return _DELIVERY_STATES.get(installation_source)


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


def _git_origin_url(repository: Path) -> str | None:
    """The checkout's ``origin`` URL, or ``None`` when it deliberately has no remote.

    A candidate-pinned fixture is built locally and never fetched from anywhere, so
    ``git remote get-url origin`` exits non-zero by design. Treating that as a probe failure
    made the deferred state unprovable; the deferred shape asserts the absence positively
    instead, and the live shape still demands the exact marketplace URL.
    """
    completed = subprocess.run(
        ["git", "-C", str(repository), "remote", "get-url", "origin"],
        text=True,
        capture_output=True,
        timeout=30,
    )
    if completed.returncode != 0:
        require(
            "No such remote" in completed.stderr,
            f"Git identity probe failed: {completed.stderr.strip()}",
        )
        return None
    return completed.stdout.strip()


def _marketplace_source(source_sha: str) -> dict[str, str]:
    return {
        "source": "git-subdir",
        "url": "https://github.com/TheGreenCedar/CodeStory.git",
        "path": "plugins/codestory",
        "sha": source_sha,
    }


def _marketplace_origin(state: _DeliveryState, marketplace_root: Path) -> dict[str, str]:
    return {
        "sourceType": state.source_type,
        "source": _MARKETPLACE_URL if state is _LIVE else str(marketplace_root),
    }


def _validate_attestation_paths(
    attestation: dict,
    state: _DeliveryState,
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
        and attestation["installation_source"] == state.installation_source
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
    state: _DeliveryState,
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
        marketplace["repository"] == state.repository
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
    if state.checkout_inside_codex_home:
        expected_root = codex_home / ".tmp" / "marketplaces" / _MARKETPLACE_NAME
        require(
            marketplace_root.is_dir()
            and marketplace_root.is_relative_to(codex_home)
            and same_existing_path(marketplace_root, expected_root),
            "Codex marketplace root is outside its isolated home",
        )
    else:
        # A local catalog is read where it was built, so it cannot be required to live inside
        # the Codex home. What it must not be is the CodeStory checkout itself: a catalog that
        # is the tree under test would make the resolve prove nothing.
        require(
            marketplace_root.is_dir()
            and not marketplace_root.is_relative_to(codex_home)
            and not marketplace_root.is_relative_to(REPOSITORY_ROOT)
            and not REPOSITORY_ROOT.is_relative_to(marketplace_root),
            "candidate-pinned marketplace fixture is the release checkout or the Codex home",
        )
    _validate_marketplace_list(marketplace, state, marketplace_root)
    plugin_source_sha = _validate_plugin_results(
        marketplace, state, marketplace_root, plugin_root, manifest
    )
    return marketplace_root, plugin_source_sha


def _validate_marketplace_list(
    marketplace: dict, state: _DeliveryState, marketplace_root: Path
) -> None:
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
                    "marketplaceSource": _marketplace_origin(state, marketplace_root),
                }
            ]
        },
        "Codex marketplace list does not match the configured snapshot",
    )


def _validate_plugin_results(
    marketplace: dict,
    state: _DeliveryState,
    marketplace_root: Path,
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
                    "marketplaceSource": _marketplace_origin(state, marketplace_root),
                    "installPolicy": "AVAILABLE",
                    "authPolicy": "ON_INSTALL",
                }
            ],
            "available": [],
        },
        "Codex plugin list does not contain exactly the enabled installed plugin",
    )
    return source_sha


def _validate_fixture_identity(
    marketplace_root: Path, plugin_source_sha: str, manifest: dict
) -> None:
    """A deferred install must resolve a fixture that says what it is, in its own bytes.

    Without this, any local git directory carrying a plausible catalog would satisfy the
    deferred shape. The marker is written by build-marketplace-fixture.mjs and names the commit
    the catalog pins, so the fixture, the catalog, and the released source all have to agree.
    """
    marker_path = marketplace_root / _FIXTURE_MARKER_FILENAME
    require(
        marker_path.is_file(),
        "local catalog resolve did not use a candidate-pinned marketplace fixture",
    )
    marker = json.loads(marker_path.read_text(encoding="utf-8"))
    require_exact_keys(
        marker,
        {"schema_version", "purpose", "pinned_commit", "plugin_version"},
        "marketplace fixture marker",
    )
    require(
        marker["schema_version"] == 1
        and marker["purpose"] == _FIXTURE_MARKER_PURPOSE
        and marker["pinned_commit"] == plugin_source_sha
        and marker["pinned_commit"] == manifest["source"]["commit"]
        and marker["plugin_version"] == manifest["release_version"],
        "marketplace fixture does not pin the exact released commit it served",
    )


def _validate_marketplace_checkout(
    codex_home: Path,
    state: _DeliveryState,
    marketplace_root: Path,
    marketplace: dict,
    plugin_source_sha: str,
    manifest: dict,
) -> str:
    config = tomllib.loads((codex_home / "config.toml").read_text(encoding="utf-8"))
    marketplace_config = config.get("marketplaces", {}).get(_MARKETPLACE_NAME)
    plugin_config = config.get("plugins", {}).get(_PLUGIN_ID)
    require(
        isinstance(marketplace_config, dict)
        and marketplace_config.get("source_type") == state.source_type
        and marketplace_config.get("source")
        == _marketplace_origin(state, marketplace_root)["source"]
        and plugin_config == {"enabled": True},
        "isolated Codex config does not record the resolved marketplace source",
    )
    if state is _LIVE:
        require(
            marketplace_config.get("ref") == marketplace["revision"],
            "isolated Codex config does not pin the immutable marketplace revision",
        )
    else:
        # A local source has no ref to pin. Recording one would be the deferred state claiming
        # a live catalog revision, so its absence is asserted rather than merely unchecked.
        require(
            "ref" not in marketplace_config,
            "deferred catalog config claims a live marketplace revision",
        )
    # Checked before the Git probes, not after: the marker is committed into the fixture, so a
    # missing or altered one also dirties the tree. Ordering it first means the failure names
    # the actual defect instead of passing for an unrelated reason.
    if state is not _LIVE:
        _validate_fixture_identity(marketplace_root, plugin_source_sha, manifest)
    marketplace_commit = _git_output(marketplace_root, "rev-parse", "HEAD")
    origin = _git_origin_url(marketplace_root)
    require(
        marketplace_commit == marketplace["revision"]
        and _git_output(marketplace_root, "status", "--porcelain") == ""
        and origin == (_MARKETPLACE_URL if state is _LIVE else None),
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
    state: _DeliveryState,
    installed_plugin_data: Path,
    plugin_root: Path,
    manifest: dict,
) -> dict:
    codex_home, plugin, marketplace = _validate_attestation_paths(
        attestation,
        state,
        installed_plugin_data,
        plugin_root,
        manifest,
    )
    marketplace_root, plugin_source_sha = _validate_marketplace_results(
        marketplace,
        state,
        codex_home,
        plugin_root,
        manifest,
    )
    marketplace_commit = _validate_marketplace_checkout(
        codex_home,
        state,
        marketplace_root,
        marketplace,
        plugin_source_sha,
        manifest,
    )
    require(
        plugin["source_commit"] == plugin_source_sha,
        "marketplace attestation source does not match the catalog pin",
    )
    package_sha256 = _validate_release_source(plugin, plugin_root, manifest)
    return {
        "schema_version": 2,
        "installation_source": state.installation_source,
        "codex_cli_version": PINNED_CODEX_CLI_VERSION,
        "marketplace_repository": state.repository,
        "marketplace_commit": marketplace_commit,
        "plugin_id": "codestory",
        "plugin_version": manifest["release_version"],
        "plugin_source_commit": plugin["source_commit"],
        "plugin_source_tree": plugin["source_tree"],
        "plugin_package_sha256": package_sha256,
    }
