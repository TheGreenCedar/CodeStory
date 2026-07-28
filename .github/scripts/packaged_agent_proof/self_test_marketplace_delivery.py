"""Catalog delivery state self-tests for the installed-runtime identity predicate.

Catalog publication is delivery, not a release gate, so a release can be proved against the
live public catalog OR against a catalog pinned to the exact published commit. These are two
distinct states and the predicate accepts two distinct shapes. What must never happen is either
one passing as the other, or an arbitrary local directory passing as the pinned fixture -- so
every assertion here that matters is a rejection.

The first version of the deferred path shipped with none of this. It attested a fixture resolve
as ``codex_marketplace_install``, wrote a temporary directory into
``marketplace.repository``, and the live predicate refused it three steps after the release tag
was already pushed. Each case below is one of the ways that failed, asserted in the
fail-closed direction.
"""

from __future__ import annotations

import argparse
import copy
import json
import shutil
import subprocess
import tempfile
from collections.abc import Callable
from pathlib import Path
from types import SimpleNamespace

from .foundation import REPOSITORY_ROOT, ProofFailure, require
from .installed_identity import installed_plugin_identity
from .marketplace_installation import (
    DEFERRED_INSTALLATION_SOURCE,
    LIVE_INSTALLATION_SOURCE,
)

_MARKETPLACE_NAME = "TheGreenCedar"
_LIVE_REPOSITORY = "TheGreenCedar/AgentPluginMarketplace"
_LIVE_URL = f"https://github.com/{_LIVE_REPOSITORY}.git"
_DEFERRED_REPOSITORY = "local:candidate-pinned-marketplace-fixture"
_MARKER_FILENAME = ".codestory-marketplace-fixture.json"
_MARKER_PURPOSE = "codestory-candidate-pinned-marketplace-fixture"
_PLUGIN_ID = f"codestory@{_MARKETPLACE_NAME}"


def _git(repository: Path, *arguments: str) -> str:
    completed = subprocess.run(
        ["git", "-C", str(repository), *arguments],
        text=True,
        capture_output=True,
        timeout=60,
    )
    require(
        completed.returncode == 0,
        f"marketplace delivery self-test git command failed: {completed.stderr.strip()}",
    )
    return completed.stdout.strip()


def _pinned_source(commit: str) -> dict[str, str]:
    return {
        "source": "git-subdir",
        "url": "https://github.com/TheGreenCedar/CodeStory.git",
        "path": "plugins/codestory",
        "sha": commit,
    }


def _manifest() -> dict:
    version = json.loads(
        (
            REPOSITORY_ROOT / "plugins" / "codestory" / ".codex-plugin" / "plugin.json"
        ).read_text(encoding="utf-8")
    )["version"]
    return {
        "release_version": version,
        "asset_target": "linux-x64",
        "source": {
            "commit": _git(REPOSITORY_ROOT, "rev-parse", "HEAD"),
            "tree": _git(REPOSITORY_ROOT, "rev-parse", "HEAD^{tree}"),
        },
    }


def _write_catalog(root: Path, commit: str) -> None:
    catalog_directory = root / ".agents" / "plugins"
    catalog_directory.mkdir(parents=True, exist_ok=True)
    catalog = {
        "name": _MARKETPLACE_NAME,
        "interface": {"displayName": _MARKETPLACE_NAME},
        "plugins": [
            {
                "name": "codestory",
                "source": _pinned_source(commit),
                "policy": {"installation": "AVAILABLE", "authentication": "ON_INSTALL"},
                "category": "Developer Tools",
            }
        ],
    }
    (catalog_directory / "marketplace.json").write_text(
        f"{json.dumps(catalog, indent=2)}\n", encoding="utf-8"
    )


def _commit_all(root: Path, message: str) -> str:
    _git(root, "add", "--all")
    _git(
        root,
        "-c",
        "user.email=self-test@codestory.invalid",
        "-c",
        "user.name=self test",
        "commit",
        "--quiet",
        "--message",
        message,
    )
    return _git(root, "rev-parse", "HEAD")


def _build_world(root: Path, deferred: bool, manifest: dict) -> dict:
    """A complete, valid installed-runtime world on disk for one delivery state."""
    commit = manifest["source"]["commit"]
    codex_home = (root / "codex-home").resolve()
    plugin_data = codex_home / "plugin-data"
    plugin_data.mkdir(parents=True)
    plugin_root = (
        codex_home
        / "plugins"
        / "cache"
        / _MARKETPLACE_NAME
        / "codestory"
        / manifest["release_version"]
    )
    plugin_root.parent.mkdir(parents=True)
    shutil.copytree(REPOSITORY_ROOT / "plugins" / "codestory", plugin_root)

    if deferred:
        marketplace_root = (root / "fixture").resolve()
        marketplace_root.mkdir(parents=True)
    else:
        marketplace_root = (codex_home / ".tmp" / "marketplaces" / _MARKETPLACE_NAME).resolve()
        marketplace_root.mkdir(parents=True)
    _write_catalog(marketplace_root, commit)
    if deferred:
        (marketplace_root / _MARKER_FILENAME).write_text(
            f"{json.dumps({
                'schema_version': 1,
                'purpose': _MARKER_PURPOSE,
                'pinned_commit': commit,
                'plugin_version': manifest['release_version'],
            }, indent=2)}\n",
            encoding="utf-8",
        )
    _git(marketplace_root, "init", "--quiet", "--initial-branch", "main")
    if not deferred:
        _git(marketplace_root, "remote", "add", "origin", _LIVE_URL)
    revision = _commit_all(marketplace_root, "catalog")

    origin = (
        {"sourceType": "local", "source": str(marketplace_root)}
        if deferred
        else {"sourceType": "git", "source": _LIVE_URL}
    )
    config = [f"[marketplaces.{_MARKETPLACE_NAME}]"]
    config.append(f'source_type = "{origin["sourceType"]}"')
    config.append(f'source = "{origin["source"]}"')
    if not deferred:
        config.append(f'ref = "{revision}"')
    config.append("")
    config.append(f'[plugins."{_PLUGIN_ID}"]')
    config.append("enabled = true")
    (codex_home / "config.toml").write_text("\n".join(config) + "\n", encoding="utf-8")

    installed_entry = {
        "pluginId": _PLUGIN_ID,
        "name": "codestory",
        "marketplaceName": _MARKETPLACE_NAME,
        "version": manifest["release_version"],
        "installed": True,
        "enabled": True,
        "source": _pinned_source(commit),
        "marketplaceSource": origin,
        "installPolicy": "AVAILABLE",
        "authPolicy": "ON_INSTALL",
    }
    attestation = {
        "schema_version": 2,
        "installation_source": (
            DEFERRED_INSTALLATION_SOURCE if deferred else LIVE_INSTALLATION_SOURCE
        ),
        "installation": {
            "codex_home": str(codex_home),
            "plugin_root": str(plugin_root),
            "plugin_data": str(plugin_data),
        },
        "plugin": {
            "id": "codestory",
            "version": manifest["release_version"],
            "source_commit": commit,
            "source_tree": manifest["source"]["tree"],
            "package_sha256": "",
        },
        "marketplace": {
            "repository": _DEFERRED_REPOSITORY if deferred else _LIVE_REPOSITORY,
            "revision": revision,
            "provenance": {
                "add": {"root": str(marketplace_root), "revision": revision},
                "list": {"root": str(marketplace_root), "revision": revision},
            },
            "codex_cli_version": f"codex-cli {_pinned_codex_cli_version()}",
            "add_result": {
                "marketplaceName": _MARKETPLACE_NAME,
                "installedRoot": str(marketplace_root),
                "alreadyAdded": False,
            },
            "list_result": {
                "marketplaces": [
                    {
                        "name": _MARKETPLACE_NAME,
                        "root": str(marketplace_root),
                        "marketplaceSource": origin,
                    }
                ]
            },
            "plugin_add_result": {
                "pluginId": _PLUGIN_ID,
                "name": "codestory",
                "marketplaceName": _MARKETPLACE_NAME,
                "version": manifest["release_version"],
                "installedPath": str(plugin_root),
                "authPolicy": "ON_INSTALL",
            },
            "plugin_list_result": {"installed": [installed_entry], "available": []},
        },
    }
    from .installation_support import directory_contract_sha256

    attestation["plugin"]["package_sha256"] = directory_contract_sha256(plugin_root)
    return {
        "attestation": attestation,
        "plugin_root": plugin_root,
        "plugin_data": plugin_data,
        "marketplace_root": marketplace_root,
        "codex_home": codex_home,
    }


def _pinned_codex_cli_version() -> str:
    from .foundation import PINNED_CODEX_CLI_VERSION

    return PINNED_CODEX_CLI_VERSION


def _verify(world: dict, attestation: dict, root: Path, manifest: dict) -> dict:
    path = root / "attestation.json"
    path.write_text(f"{json.dumps(attestation, indent=2)}\n", encoding="utf-8")
    args = argparse.Namespace(
        proof_tier="installed_runtime",
        installed_plugin_attestation=path,
        installed_plugin_data=world["plugin_data"],
        archive=None,
        candidate_producer_repository=None,
        candidate_producer_workflow_path=None,
        candidate_producer_run_id=None,
        candidate_producer_run_attempt=None,
        candidate_artifact_name=None,
    )
    return installed_plugin_identity(args, world["plugin_root"], manifest)


def _reject(
    world: dict,
    root: Path,
    manifest: dict,
    description: str,
    mutate: Callable[[dict], None],
) -> None:
    attestation = copy.deepcopy(world["attestation"])
    mutate(attestation)
    try:
        _verify(world, attestation, root, manifest)
    except ProofFailure:
        return
    raise ProofFailure(
        f"installed-runtime identity accepted {description}"
    )


def _run_deferred_self_tests(root: Path, manifest: dict) -> None:
    world = _build_world(root / "deferred", deferred=True, manifest=manifest)
    identity = _verify(world, world["attestation"], root, manifest)
    require(
        identity["installation_source"] == DEFERRED_INSTALLATION_SOURCE
        and identity["marketplace_repository"] == _DEFERRED_REPOSITORY,
        "a deferred catalog resolve did not record its own installer identity",
    )
    require(
        identity["plugin_source_commit"] == manifest["source"]["commit"]
        and identity["plugin_source_tree"] == manifest["source"]["tree"],
        "a deferred catalog resolve did not bind the released plugin source",
    )

    def relabel_as_live(attestation: dict) -> None:
        attestation["installation_source"] = LIVE_INSTALLATION_SOURCE
        attestation["marketplace"]["repository"] = _LIVE_REPOSITORY

    _reject(
        world,
        root,
        manifest,
        "a fixture resolve relabelled as a live public-catalog install",
        relabel_as_live,
    )
    _reject(
        world,
        root,
        manifest,
        "a deferred resolve claiming the live catalog repository",
        lambda attestation: attestation["marketplace"].update(
            repository=_LIVE_REPOSITORY
        ),
    )
    _reject(
        world,
        root,
        manifest,
        "a deferred resolve claiming a git-backed marketplace source",
        lambda attestation: attestation["marketplace"]["list_result"]["marketplaces"][
            0
        ].update(marketplaceSource={"sourceType": "git", "source": _LIVE_URL}),
    )
    _reject(
        world,
        root,
        manifest,
        "an installer identity no delivery state defines",
        lambda attestation: attestation.update(
            installation_source="codex_marketplace_install_v3"
        ),
    )
    _reject(
        world,
        root,
        manifest,
        "a deferred resolve whose catalog is the release checkout itself",
        lambda attestation: attestation["marketplace"]["add_result"].update(
            installedRoot=str(REPOSITORY_ROOT)
        ),
    )

    marker = world["marketplace_root"] / _MARKER_FILENAME
    saved = marker.read_bytes()
    marker.unlink()
    _commit_all(world["marketplace_root"], "drop the fixture marker")
    absent = copy.deepcopy(world["attestation"])
    revision = _git(world["marketplace_root"], "rev-parse", "HEAD")
    _restamp(absent, revision)
    try:
        _verify(world, absent, root, manifest)
        raise ProofFailure(
            "installed-runtime identity accepted a local catalog carrying no fixture marker"
        )
    except ProofFailure as exc:
        require(
            "candidate-pinned marketplace fixture" in str(exc),
            f"a marker-less local catalog was refused for the wrong reason: {exc}",
        )
    marker.write_bytes(saved)
    json_marker = json.loads(saved)
    json_marker["pinned_commit"] = "0" * 40
    marker.write_text(f"{json.dumps(json_marker, indent=2)}\n", encoding="utf-8")
    _commit_all(world["marketplace_root"], "pin a different commit")
    mismatched = copy.deepcopy(world["attestation"])
    _restamp(mismatched, _git(world["marketplace_root"], "rev-parse", "HEAD"))
    try:
        _verify(world, mismatched, root, manifest)
        raise ProofFailure(
            "installed-runtime identity accepted a fixture pinning another commit"
        )
    except ProofFailure as exc:
        require(
            "does not pin the exact released commit" in str(exc),
            f"a mispinned fixture was refused for the wrong reason: {exc}",
        )

    config = world["codex_home"] / "config.toml"
    saved_config = config.read_text(encoding="utf-8")
    config.write_text(
        saved_config.replace(
            "source_type =", f'ref = "{world["attestation"]["marketplace"]["revision"]}"\nsource_type ='
        ),
        encoding="utf-8",
    )
    marker.write_bytes(saved)
    _commit_all(world["marketplace_root"], "restore the fixture marker")
    claimed = copy.deepcopy(world["attestation"])
    _restamp(claimed, _git(world["marketplace_root"], "rev-parse", "HEAD"))
    try:
        _verify(world, claimed, root, manifest)
        raise ProofFailure(
            "installed-runtime identity accepted a deferred config claiming a live ref"
        )
    except ProofFailure as exc:
        require(
            "claims a live marketplace revision" in str(exc),
            f"a deferred config claiming a live ref was refused for the wrong reason: {exc}",
        )
    config.write_text(saved_config, encoding="utf-8")

    _git(world["marketplace_root"], "remote", "add", "origin", _LIVE_URL)
    dressed = copy.deepcopy(world["attestation"])
    _restamp(dressed, _git(world["marketplace_root"], "rev-parse", "HEAD"))
    try:
        _verify(world, dressed, root, manifest)
        raise ProofFailure(
            "installed-runtime identity accepted a fixture wearing the live marketplace origin"
        )
    except ProofFailure:
        pass


def _restamp(attestation: dict, revision: str) -> None:
    marketplace = attestation["marketplace"]
    marketplace["revision"] = revision
    for operation in ("add", "list"):
        marketplace["provenance"][operation]["revision"] = revision


def _run_live_self_tests(root: Path, manifest: dict) -> None:
    world = _build_world(root / "live", deferred=False, manifest=manifest)
    identity = _verify(world, world["attestation"], root, manifest)
    require(
        identity["installation_source"] == LIVE_INSTALLATION_SOURCE
        and identity["marketplace_repository"] == _LIVE_REPOSITORY,
        "a live catalog resolve did not record the public marketplace identity",
    )
    _reject(
        world,
        root,
        manifest,
        "a live install naming the deferred fixture repository",
        lambda attestation: attestation["marketplace"].update(
            repository=_DEFERRED_REPOSITORY
        ),
    )
    _reject(
        world,
        root,
        manifest,
        "a live install relabelled as a deferred fixture resolve",
        lambda attestation: attestation.update(
            installation_source=DEFERRED_INSTALLATION_SOURCE
        ),
    )
    # The live shape is not relaxed to admit a local source: this is the check the deferred
    # state was originally, wrongly, expected to satisfy.
    _reject(
        world,
        root,
        manifest,
        "a live install whose marketplace list reports a local source",
        lambda attestation: attestation["marketplace"]["list_result"]["marketplaces"][
            0
        ].update(
            marketplaceSource={
                "sourceType": "local",
                "source": attestation["marketplace"]["add_result"]["installedRoot"],
            }
        ),
    )


def _run_retained_provenance_self_tests() -> None:
    """The retained evidence verifier must bind each installer identity to its own repository.

    The identity predicate is only the first reader. Retained qualification evidence names the
    marketplace repository too, and it hard-coded the live one -- so a deferred release would
    have been refused here even after the install itself was accepted.
    """
    from . import qualification_retained_provenance as retained
    from .foundation import PINNED_CODEX_CLI_VERSION

    manifest = {
        "release_version": "0.16.2",
        "source": {"tree": "a" * 40, "commit": "b" * 40},
    }
    contract = SimpleNamespace(manifest=manifest)
    runtime = {"build_source": "github_release", "repo_ref": "v0.16.2"}

    def plugin(installation_source: str, repository: str) -> dict:
        return {
            "installation_source": installation_source,
            "marketplace_repository": repository,
            "codex_cli_version": PINNED_CODEX_CLI_VERSION,
            "marketplace_commit": "c" * 40,
            "plugin_source_commit": "b" * 40,
            "plugin_source_tree": "a" * 40,
        }

    for source, repository in (
        (LIVE_INSTALLATION_SOURCE, _LIVE_REPOSITORY),
        (DEFERRED_INSTALLATION_SOURCE, _DEFERRED_REPOSITORY),
    ):
        retained._verify_marketplace_provenance(contract, plugin(source, repository), runtime)

    # Each identity may name only its own repository, and an identity no state declares is not a
    # delivery state at all.
    for description, source, repository in (
        ("a deferred fixture claiming the live catalog repository",
         DEFERRED_INSTALLATION_SOURCE, _LIVE_REPOSITORY),
        ("a live install claiming the fixture repository",
         LIVE_INSTALLATION_SOURCE, _DEFERRED_REPOSITORY),
        ("an installer identity no delivery state declares",
         "codex_marketplace_install_v3", _LIVE_REPOSITORY),
    ):
        try:
            retained._verify_marketplace_provenance(
                contract, plugin(source, repository), runtime
            )
        except ProofFailure:
            continue
        raise ProofFailure(f"retained installed evidence accepted {description}")


def _run_shared_identity_self_tests() -> None:
    """The producer and the verifier must name the two states identically.

    `.github/scripts/marketplace-delivery-identity.mjs` writes the installer identity and the
    attestation repository; this module's predicate decides which shape to accept from them. If
    the two ever drift, a real release resolves through a Codex install the predicate refuses --
    which is precisely the failure this whole path was repaired for, and it would surface only
    after the tag was already pushed.
    """
    source = (
        REPOSITORY_ROOT / ".github" / "scripts" / "marketplace-delivery-identity.mjs"
    ).read_text(encoding="utf-8")
    for name, value in (
        ("LIVE_INSTALLATION_SOURCE", LIVE_INSTALLATION_SOURCE),
        ("DEFERRED_INSTALLATION_SOURCE", DEFERRED_INSTALLATION_SOURCE),
        ("LIVE_MARKETPLACE_REPOSITORY", _LIVE_REPOSITORY),
        ("DEFERRED_MARKETPLACE_REPOSITORY", _DEFERRED_REPOSITORY),
        ("FIXTURE_MARKER_FILENAME", _MARKER_FILENAME),
        ("FIXTURE_MARKER_PURPOSE", _MARKER_PURPOSE),
    ):
        require(
            f'export const {name} = "{value}";' in source,
            f"marketplace delivery identity {name} differs between the producer and the verifier",
        )


def run_marketplace_delivery_self_tests() -> None:
    manifest = _manifest()
    with tempfile.TemporaryDirectory(prefix="codestory-marketplace-delivery-") as raw:
        root = Path(raw).resolve()
        _run_deferred_self_tests(root, manifest)
        _run_live_self_tests(root, manifest)
    _run_retained_provenance_self_tests()
    _run_shared_identity_self_tests()
