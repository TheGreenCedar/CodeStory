"""Packaged runtime proof project, environment, and host setup."""

from __future__ import annotations

import argparse
import hashlib
import shutil
from pathlib import Path

from .contracts import sha256, write_private_json
from .foundation import TARGET_CONTRACTS, require
from .installation import create_second_repository, installed_plugin_identity, qualification_environment
from .process import McpProcess, process_start_identity
from .runtime_bootstrap_types import HostPair, RuntimeSetup

def _proof_projects(args: argparse.Namespace, root: Path) -> tuple[Path, Path, str]:
    require(args.project is not None, "--project is required for runtime proof")
    project_a = args.project.resolve()
    require(project_a.is_dir(), f"first proof repository does not exist: {project_a}")
    require(
        len(args.additional_project) == len(args.additional_query),
        "each --additional-project requires one --additional-query",
    )
    if args.additional_project:
        require(
            len(args.additional_project) == 1,
            "two-host proof accepts exactly one --additional-project",
        )
        project_b = args.additional_project[0].resolve()
        query_b = args.additional_query[0]
    else:
        project_b = create_second_repository(root)
        query_b = "shared_engine_probe"
    require(project_b.is_dir(), f"second proof repository does not exist: {project_b}")
    require(project_a != project_b, "two-host proof requires different repositories")
    return project_a, project_b, query_b


def _runtime_environment(
    args: argparse.Namespace,
    cli: Path,
    env: dict[str, str],
    root: Path,
    provenance: dict | None,
) -> tuple[dict[str, str], dict]:
    qualified, control = qualification_environment(root, env)
    qualified.pop("CODESTORY_CLI", None)
    if args.proof_tier != "installed_runtime":
        qualified["CODESTORY_CLI"] = str(cli)
        return qualified, control
    qualified["CODESTORY_PLUGIN_DATA"] = str(args.installed_plugin_data.resolve())
    if provenance["installation_source"] == "candidate_archive":
        archive_sha256 = sha256(args.archive)
        qualified["CODESTORY_PLUGIN_CANDIDATE_ARCHIVE_SHA256"] = archive_sha256
        write_private_json(
            Path(qualified["CODESTORY_EMBED_QUALIFICATION_DIR"])
            / "candidate-managed-install.json",
            {
                "schema_version": 1,
                "purpose": "codestory-candidate-managed-install",
                "archive_sha256": archive_sha256,
                "qualification_nonce_sha256": hashlib.sha256(
                    qualified["CODESTORY_EMBED_QUALIFICATION_NONCE"].encode("ascii")
                ).hexdigest(),
            },
        )
    return qualified, control


def _runtime_setup(
    args: argparse.Namespace,
    cli: Path,
    env: dict[str, str],
    root: Path,
    manifest: dict,
    cleanup_control: dict,
) -> RuntimeSetup:
    require(args.plugin_handoff, "runtime proof requires the ordinary packaged plugin handoff")
    require(args.plugin_root is not None, "--plugin-handoff requires --plugin-root")
    project_a, project_b, query_b = _proof_projects(args, root)
    plugin_root = args.plugin_root.resolve()
    provenance = (
        installed_plugin_identity(args, plugin_root, manifest)
        if args.proof_tier == "installed_runtime"
        else None
    )
    launcher = plugin_root / "scripts" / "codestory-mcp.cjs"
    require(launcher.is_file(), f"plugin launcher is missing: {launcher}")
    node_raw = shutil.which("node")
    require(node_raw is not None, "packaged plugin proof requires Node.js for the host launcher")
    node = Path(node_raw)
    qualified_env, qualification_control = _runtime_environment(
        args,
        cli,
        env,
        root,
        provenance,
    )
    cleanup_control.update(
        {
            "qualification_cli": str(cli.resolve()),
            "qualification_directory": qualified_env["CODESTORY_EMBED_QUALIFICATION_DIR"],
            "qualification_nonce": qualified_env["CODESTORY_EMBED_QUALIFICATION_NONCE"],
            "plugin_cli_archive_sha256": None,
            "projects": [str(project_a), str(project_b)],
        }
    )
    embedded_models = Path(qualified_env["CODESTORY_CACHE_ROOT"]) / "embedded-models"
    require(not embedded_models.exists(), "isolated proof cache was not empty before first use")
    return RuntimeSetup(
        project_a=project_a,
        project_b=project_b,
        query_b=query_b,
        plugin_root=plugin_root,
        provenance=provenance,
        node=node,
        qualified_env=qualified_env,
        qualification_control=qualification_control,
        target_os=TARGET_CONTRACTS[manifest["asset_target"]]["target_os"],
        command=[str(node), str(launcher)],
        embedded_models=embedded_models,
    )


def _start_hosts(args: argparse.Namespace, setup: RuntimeSetup) -> HostPair:
    host_a = McpProcess(
        setup.command,
        env=setup.qualified_env,
        cwd=setup.project_a,
        timeout=args.timeout_secs,
    )
    host_b = McpProcess(
        setup.command,
        env=setup.qualified_env,
        cwd=setup.project_b,
        timeout=args.timeout_secs,
    )
    start_a = process_start_identity(host_a.process.pid)
    start_b = process_start_identity(host_b.process.pid)
    require(
        (host_a.process.pid, start_a) != (host_b.process.pid, start_b),
        "plugin hosts are not independent processes",
    )
    return HostPair(host_a, host_b, start_a, start_b)
