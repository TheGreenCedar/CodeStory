"""Installed plugin identity dispatch and candidate attestation verification."""

from __future__ import annotations

import argparse
import json
import subprocess
from pathlib import Path

from .contracts import require_exact_keys, sha256
from .foundation import REPOSITORY_ROOT, ProofFailure, require
from .installation_support import directory_contract_sha256, same_existing_path
from .marketplace_installation import marketplace_installed_plugin_identity


def _reject_source_checkout(plugin_root: Path) -> None:
    source_plugin_root = REPOSITORY_ROOT / "plugins" / "codestory"
    require(
        not same_existing_path(plugin_root, source_plugin_root),
        "installed_runtime proof rejects the repository-source plugin root",
    )
    completed = subprocess.run(
        ["git", "-C", str(plugin_root), "rev-parse", "--show-toplevel"],
        text=True,
        capture_output=True,
        timeout=20,
    )
    if completed.returncode == 0:
        checkout = Path(completed.stdout.strip())
        require(
            not (
                (checkout / "Cargo.toml").is_file()
                and (checkout / "crates/codestory-cli").is_dir()
            ),
            "installed_runtime proof rejects a plugin launched from a CodeStory source checkout",
        )


def _load_attestation(path: Path) -> dict:
    try:
        attestation = json.loads(path.read_text(encoding="utf-8"))
    except json.JSONDecodeError as exc:
        raise ProofFailure(
            f"installed plugin attestation is not valid JSON: {exc}"
        ) from exc
    require(
        isinstance(attestation, dict) and attestation.get("schema_version") == 2,
        "installed plugin attestation schema is unsupported",
    )
    return attestation


def _validate_candidate_attestation_shape(
    attestation: dict,
) -> tuple[dict, dict, dict]:
    require_exact_keys(
        attestation,
        {
            "schema_version",
            "installation_source",
            "installation",
            "plugin",
            "candidate",
        },
        "candidate install attestation",
    )
    installation = attestation["installation"]
    plugin = attestation["plugin"]
    candidate = attestation["candidate"]
    require_exact_keys(
        installation,
        {"plugin_root", "plugin_data"},
        "candidate installation paths",
    )
    require_exact_keys(
        plugin,
        {"id", "version", "source_commit", "source_tree", "package_sha256"},
        "candidate installed plugin",
    )
    require_exact_keys(
        candidate,
        {"archive_sha256", "asset_target", "producer"},
        "candidate install producer",
    )
    return installation, plugin, candidate


def _expected_candidate(args: argparse.Namespace, manifest: dict) -> dict:
    return {
        "archive_sha256": sha256(args.archive),
        "asset_target": manifest["asset_target"],
        "producer": {
            "repository": args.candidate_producer_repository,
            "workflow_path": args.candidate_producer_workflow_path,
            "run_id": args.candidate_producer_run_id,
            "run_attempt": args.candidate_producer_run_attempt,
            "artifact_name": args.candidate_artifact_name,
        },
    }


def _candidate_installed_plugin_identity(
    args: argparse.Namespace,
    attestation: dict,
    plugin_root: Path,
    manifest: dict,
) -> dict:
    installation, plugin, candidate = _validate_candidate_attestation_shape(attestation)
    package_sha256 = directory_contract_sha256(plugin_root)
    expected_plugin = {
        "id": "codestory",
        "version": manifest["release_version"],
        "source_commit": manifest["source"]["commit"],
        "source_tree": manifest["source"]["tree"],
        "package_sha256": package_sha256,
    }
    require(
        attestation["installation_source"] == "candidate_archive"
        and same_existing_path(Path(installation["plugin_root"]), plugin_root)
        and same_existing_path(
            Path(installation["plugin_data"]),
            args.installed_plugin_data,
        )
        and plugin == expected_plugin
        and candidate == _expected_candidate(args, manifest),
        "candidate attestation does not bind the exact archive and source tree",
    )
    return {
        "schema_version": 2,
        "installation_source": "candidate_archive",
        "plugin_id": "codestory",
        "plugin_version": plugin["version"],
        "plugin_source_commit": plugin["source_commit"],
        "plugin_package_sha256": package_sha256,
        "plugin_source_tree": plugin["source_tree"],
        "candidate_archive_sha256": candidate["archive_sha256"],
        "candidate_asset_target": candidate["asset_target"],
        "producer": candidate["producer"],
    }


def installed_plugin_identity(
    args: argparse.Namespace,
    plugin_root: Path,
    manifest: dict,
) -> dict:
    require(
        args.proof_tier == "installed_runtime",
        "installed plugin identity is valid only at installed_runtime tier",
    )
    require(
        args.installed_plugin_attestation is not None
        and args.installed_plugin_attestation.is_file()
        and args.installed_plugin_data is not None
        and args.installed_plugin_data.is_dir(),
        "installed_runtime proof requires one attestation and its plugin data directory",
    )
    _reject_source_checkout(plugin_root)
    attestation = _load_attestation(args.installed_plugin_attestation)
    if attestation.get("installation_source") == "codex_marketplace_install":
        return marketplace_installed_plugin_identity(
            attestation,
            args.installed_plugin_data,
            plugin_root,
            manifest,
        )
    return _candidate_installed_plugin_identity(
        args,
        attestation,
        plugin_root,
        manifest,
    )
