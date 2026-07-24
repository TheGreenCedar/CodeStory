"""One-project packaged and installed ground proof."""

from __future__ import annotations

import argparse
import hashlib
import shutil
from pathlib import Path

from .contract_primitives import (
    assert_retained_json_privacy,
    require_nonempty_string,
    retained_mcp_transcript,
    retained_runtime_evidence,
    sha256,
    write_json,
    write_private_json,
)
from .foundation import LOWER_TIER_NONCLAIMS, require
from .installation_support import assert_no_legacy_state, qualification_environment
from .installed_identity import installed_plugin_identity
from .managed_runtime import verify_managed_runtime_status
from .subprocess_control import McpProcess


def _qualification_environment(
    args: argparse.Namespace,
    cli: Path,
    env: dict[str, str],
    root: Path,
    provenance: dict | None,
) -> dict[str, str]:
    qualified, _control = qualification_environment(root, env)
    qualified.pop("CODESTORY_CLI", None)
    if args.proof_tier != "installed_runtime":
        qualified["CODESTORY_CLI"] = str(cli)
        return qualified
    require(
        args.installed_plugin_data is not None,
        "installed ground-only proof requires --installed-plugin-data",
    )
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
    return qualified


def _managed_ground_runtime(
    args: argparse.Namespace,
    host: McpProcess,
    project: Path,
    plugin_root: Path,
    cli: Path,
    manifest: dict,
    provenance: dict,
) -> tuple[dict, Path]:
    status = host.status(project, "installed-ground-status")
    managed_runtime = verify_managed_runtime_status(
        status,
        plugin_root=plugin_root,
        manifest=manifest,
        archive_sha256=sha256(args.archive),
    )
    if provenance["installation_source"] == "candidate_archive":
        require(
            managed_runtime["build_source"] == "candidate_archive"
            and managed_runtime["repo_ref"] == manifest["source"]["commit"],
            "candidate installed ground did not launch the staged candidate archive",
        )
    else:
        require(
            managed_runtime["build_source"] == "github_release"
            and managed_runtime["repo_ref"] == f"v{manifest['release_version']}",
            "marketplace installed ground did not launch the published release archive",
        )
    managed_binary = Path(
        require_nonempty_string(
            status["plugin_runtime"].get("managed_binary_path"),
            "installed plugin_runtime.managed_binary_path",
        )
    ).resolve()
    require(
        managed_binary.is_relative_to(args.installed_plugin_data.resolve()),
        "installed managed executable is outside the installed plugin data root",
    )
    require(
        managed_binary != cli.resolve(),
        "installed ground proof used the unpacked package executable as its managed runtime",
    )
    return managed_runtime, managed_binary


def _run_ground(
    args: argparse.Namespace,
    host: McpProcess,
    project: Path,
    plugin_root: Path,
    cli: Path,
    manifest: dict,
    provenance: dict | None,
) -> tuple[int, dict | None, Path | None]:
    host.initialize()
    ground_response, ground_attempts = host.tool_until_ready(
        "ground",
        {"project": str(project), "budget": "strict"},
        "installed-ground",
    )
    ground = ground_response["result"]["structuredContent"]
    require(
        isinstance(ground, dict) and ground,
        f"installed runtime ground returned no structured result: {ground!r}",
    )
    if args.proof_tier != "installed_runtime":
        return ground_attempts, None, None
    managed_runtime, managed_binary = _managed_ground_runtime(
        args,
        host,
        project,
        plugin_root,
        cli,
        manifest,
        provenance,
    )
    return ground_attempts, managed_runtime, managed_binary


def _ground_result(
    *,
    attempts: int,
    provenance: dict | None,
    managed_runtime: dict | None,
    managed_binary: Path | None,
    cli: Path,
    project: Path,
    plugin_root: Path,
    root: Path,
    qualified_env: dict[str, str],
) -> dict:
    qualification_cli = managed_binary if managed_binary is not None else cli.resolve()
    forbidden_values = [
        str(project),
        str(plugin_root),
        str(cli.resolve()),
        str(root.resolve()),
        qualified_env["CODESTORY_EMBED_QUALIFICATION_DIR"],
        qualified_env["CODESTORY_EMBED_QUALIFICATION_NONCE"],
    ]
    if managed_binary is not None:
        forbidden_values.append(str(managed_binary))
    return {
        "ground": {
            "status": "pass",
            "attempts": attempts,
            "project_bound": True,
            "response_nonempty": True,
        },
        "installed_plugin": provenance,
        "managed_runtime": managed_runtime,
        "_qualification_cli_path": str(qualification_cli),
        "_qualification_projects": [str(project)],
        "_qualification_forbidden_values": forbidden_values,
        "nonclaims": {
            claim: {
                "claimed": False,
                "reason": "installed ground proof does not establish this claim",
            }
            for claim in sorted(LOWER_TIER_NONCLAIMS)
        },
    }


def prove_ground_only_runtime(
    args: argparse.Namespace,
    cli: Path,
    env: dict[str, str],
    root: Path,
    out_dir: Path,
    manifest: dict,
) -> dict:
    require(
        args.plugin_handoff,
        "ground-only proof requires the ordinary packaged plugin handoff",
    )
    require(args.plugin_root is not None, "--plugin-handoff requires --plugin-root")
    require(args.project is not None, "--project is required for ground-only proof")
    require(
        not args.additional_project and not args.additional_query,
        "ground-only proof accepts exactly one project",
    )
    project = args.project.resolve()
    require(project.is_dir(), f"ground-only proof repository does not exist: {project}")
    plugin_root = args.plugin_root.resolve()
    provenance = (
        installed_plugin_identity(args, plugin_root, manifest)
        if args.proof_tier == "installed_runtime"
        else None
    )
    launcher = plugin_root / "scripts" / "codestory-mcp.cjs"
    require(launcher.is_file(), f"plugin launcher is missing: {launcher}")
    node = shutil.which("node")
    require(
        node is not None, "packaged plugin proof requires Node.js for the host launcher"
    )
    qualified_env = _qualification_environment(args, cli, env, root, provenance)
    host = McpProcess(
        [node, str(launcher)],
        env=qualified_env,
        cwd=project,
        timeout=args.timeout_secs,
    )
    try:
        attempts, managed_runtime, managed_binary = _run_ground(
            args,
            host,
            project,
            plugin_root,
            cli,
            manifest,
            provenance,
        )
        result = _ground_result(
            attempts=attempts,
            provenance=provenance,
            managed_runtime=managed_runtime,
            managed_binary=managed_binary,
            cli=cli,
            project=project,
            plugin_root=plugin_root,
            root=root,
            qualified_env=qualified_env,
        )
    finally:
        write_json(
            out_dir / "plugin-ground-mcp.json",
            retained_mcp_transcript(host.transcript),
        )
        host.close()
    assert_no_legacy_state(Path(qualified_env["CODESTORY_CACHE_ROOT"]))
    public_runtime_evidence = out_dir / "installed-ground-proof.json"
    write_json(public_runtime_evidence, retained_runtime_evidence(result))
    forbidden_values = result.get("_qualification_forbidden_values", [])
    for public_artifact in (
        out_dir / "plugin-ground-mcp.json",
        public_runtime_evidence,
    ):
        assert_retained_json_privacy(public_artifact, forbidden_values)
    return result
