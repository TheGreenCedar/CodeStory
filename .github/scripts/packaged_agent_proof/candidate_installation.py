"""Candidate archive installation preparation."""

from __future__ import annotations

import argparse
import re
import shutil
import subprocess
import tempfile
from pathlib import Path

from .archive import expected_archive_digest, find_cli, load_native_manifest, unpack_archive
from .contracts import sha256, write_json
from .foundation import CANDIDATE_PRODUCER_WORKFLOW_PATHS, REPOSITORY_ROOT, require
from .installation_support import directory_contract_sha256, same_existing_path


def _candidate_producer(args: argparse.Namespace, archive: Path) -> dict:
    producer = {
        "repository": args.candidate_producer_repository,
        "workflow_path": args.candidate_producer_workflow_path,
        "run_id": args.candidate_producer_run_id,
        "run_attempt": args.candidate_producer_run_attempt,
        "artifact_name": args.candidate_artifact_name,
    }
    require(
        producer["repository"] == "TheGreenCedar/CodeStory"
        and producer["workflow_path"] in CANDIDATE_PRODUCER_WORKFLOW_PATHS
        and isinstance(producer["run_id"], str)
        and re.fullmatch(r"[1-9][0-9]*", producer["run_id"]) is not None
        and isinstance(producer["run_attempt"], str)
        and re.fullmatch(r"[1-9][0-9]*", producer["run_attempt"]) is not None
        and producer["artifact_name"] == archive.name,
        "candidate install producer identity is missing or is not an authenticated "
        "release workflow artifact",
    )
    return producer


def _git_output(*arguments: str) -> str:
    completed = subprocess.run(
        ["git", *arguments],
        cwd=REPOSITORY_ROOT,
        text=True,
        capture_output=True,
        timeout=30,
    )
    require(
        completed.returncode == 0,
        f"candidate install Git identity probe failed: {completed.stderr.strip()}",
    )
    return completed.stdout.strip()


def _verify_candidate_checkout(source_plugin: Path, manifest: dict) -> None:
    require(
        same_existing_path(source_plugin, REPOSITORY_ROOT / "plugins" / "codestory"),
        "candidate install plugin source is not the checked-in CodeStory plugin",
    )
    require(
        _git_output("rev-parse", "HEAD") == manifest["source"]["commit"]
        and _git_output("rev-parse", "HEAD^{tree}") == manifest["source"]["tree"],
        "candidate plugin checkout does not match the packaged source commit and tree",
    )
    require(
        _git_output("status", "--porcelain", "--untracked-files=all") == "",
        "candidate plugin checkout contains tracked or untracked source drift",
    )


def _expected_archive_name(version: str, asset_target: str) -> str:
    suffix = "zip" if asset_target.startswith("windows-") else "tar.gz"
    return f"codestory-cli-v{version}-{asset_target}.{suffix}"


def _managed_manifest(
    archive: Path,
    manifest: dict,
    relative_cli: str,
    version: str,
) -> dict:
    archive_sha256 = sha256(archive)
    return {
        "path": relative_cli,
        "sha256": manifest["binary"]["sha256"],
        "version": version,
        "build_source": "candidate_archive",
        "repo_ref": manifest["source"]["commit"],
        "archive": archive.name,
        "archive_url": f"candidate-archive:{archive_sha256}",
        "archive_sha256": archive_sha256,
        "target": manifest["asset_target"],
        "stdio_initialize_verified": True,
        "provisioned_at": f"candidate-proof:{manifest['source']['commit']}",
    }


def _stage_candidate_installation(
    archive: Path,
    source_plugin: Path,
    plugin_output: Path,
    data_output: Path,
    version: str,
) -> dict:
    with tempfile.TemporaryDirectory(prefix="codestory-candidate-install-") as raw:
        unpacked = Path(raw) / "unpacked"
        unpack_archive(archive, unpacked)
        cli = find_cli(unpacked)
        manifest = load_native_manifest(unpacked, cli, version)
        _verify_candidate_checkout(source_plugin, manifest)
        require(
            archive.name == _expected_archive_name(version, manifest["asset_target"]),
            "candidate install archive name does not match its package target",
        )
        shutil.copytree(source_plugin, plugin_output)
        version_root = data_output / "codestory-cli" / version
        shutil.copytree(unpacked, version_root)
        write_json(
            version_root / "manifest.json",
            _managed_manifest(
                archive,
                manifest,
                cli.relative_to(unpacked).as_posix(),
                version,
            ),
        )
        return manifest


def _candidate_attestation(
    args: argparse.Namespace,
    archive: Path,
    plugin_output: Path,
    data_output: Path,
    manifest: dict,
    producer: dict,
) -> dict:
    return {
        "schema_version": 2,
        "installation_source": "candidate_archive",
        "installation": {
            "plugin_root": str(plugin_output),
            "plugin_data": str(data_output),
        },
        "plugin": {
            "id": "codestory",
            "version": args.expected_version,
            "source_commit": manifest["source"]["commit"],
            "source_tree": manifest["source"]["tree"],
            "package_sha256": directory_contract_sha256(plugin_output),
        },
        "candidate": {
            "archive_sha256": sha256(archive),
            "asset_target": manifest["asset_target"],
            "producer": producer,
        },
    }


def prepare_candidate_installed_proof(args: argparse.Namespace) -> dict:
    require(
        args.archive is not None
        and args.checksum_file is not None
        and args.expected_version is not None
        and args.plugin_root is not None
        and args.candidate_plugin_root_output is not None
        and args.candidate_plugin_data_output is not None
        and args.installed_plugin_attestation_output is not None,
        "candidate install preparation requires archive, checksum, version, plugin source, "
        "plugin/data outputs, and attestation output",
    )
    archive = args.archive.resolve()
    source_plugin = args.plugin_root.resolve()
    plugin_output = args.candidate_plugin_root_output.resolve()
    data_output = args.candidate_plugin_data_output.resolve()
    attestation_output = args.installed_plugin_attestation_output.resolve()
    producer = _candidate_producer(args, archive)
    require(
        sha256(archive) == expected_archive_digest(args.checksum_file.resolve(), archive),
        "candidate install archive checksum mismatch",
    )
    require(
        source_plugin.is_dir()
        and not plugin_output.exists()
        and not data_output.exists()
        and not attestation_output.exists(),
        "candidate install outputs must be absent and the source plugin must exist",
    )
    manifest = _stage_candidate_installation(
        archive,
        source_plugin,
        plugin_output,
        data_output,
        args.expected_version,
    )
    write_json(
        attestation_output,
        _candidate_attestation(
            args,
            archive,
            plugin_output,
            data_output,
            manifest,
            producer,
        ),
    )
    return {
        "plugin_root": str(plugin_output),
        "plugin_data": str(data_output),
        "attestation": str(attestation_output),
        "source": manifest["source"],
        "archive_sha256": sha256(archive),
        "asset_target": manifest["asset_target"],
    }
