"""Managed-runtime, live retrieval, continuity, and model proof phases."""

from __future__ import annotations

import argparse
from pathlib import Path

from .contract_primitives import (
    require_nonempty_string,
    retained_mcp_transcript,
    sha256,
    write_json,
)
from .foundation import require
from .installation_support import run_parallel, same_existing_path
from .managed_layout import verify_flat_managed_layout
from .managed_runtime import verify_managed_runtime_status
from .memory_observation import capture_five_process_memory
from .process_identity import process_start_identity
from .runtime_bootstrap_types import ColdProof, ContinuityProof, HostPair, RuntimeSetup
from .server_engine_identity import engine_identity
from .server_identity import server_snapshot
from .subprocess_control import McpProcess

_CALIBRATION_LIVE_QUERY_SUFFIX = " calibration live encode verification"


def _live_project_b_query(args: argparse.Namespace, setup: RuntimeSetup) -> str:
    if args.proof_tier == "calibration":
        # The cold phase already queried setup.query_b. Reusing it here can hit
        # both the retrieval-result and embedding caches without exercising the
        # resident native encoder whose counter this phase verifies.
        return f"{setup.query_b}{_CALIBRATION_LIVE_QUERY_SUFFIX}"
    return setup.query_b


def _managed_runtime(
    args: argparse.Namespace,
    cli: Path,
    setup: RuntimeSetup,
    cold: ColdProof,
    manifest: dict,
) -> tuple[dict | None, Path | None]:
    if args.proof_tier != "installed_runtime":
        return None, None
    archive_sha256 = sha256(args.archive)
    managed = verify_managed_runtime_status(
        cold.status_a,
        plugin_root=setup.plugin_root,
        manifest=manifest,
        archive_sha256=archive_sha256,
    )
    require(
        verify_managed_runtime_status(
            cold.status_b,
            plugin_root=setup.plugin_root,
            manifest=manifest,
            archive_sha256=archive_sha256,
        )
        == managed,
        "independent installed plugin hosts reported different managed runtime provenance",
    )
    if setup.provenance["installation_source"] == "candidate_archive":
        require(
            managed["build_source"] == "candidate_archive"
            and managed["repo_ref"] == manifest["source"]["commit"],
            "candidate installed proof did not launch the staged candidate archive",
        )
    else:
        require(
            managed["build_source"] == "github_release"
            and managed["repo_ref"] == f"v{manifest['release_version']}",
            "marketplace installed proof did not launch the published release archive",
        )
    managed_binary = Path(
        require_nonempty_string(
            cold.status_a["plugin_runtime"].get("managed_binary_path"),
            "installed plugin_runtime.managed_binary_path",
        )
    ).resolve()
    require(
        managed_binary.is_relative_to(args.installed_plugin_data.resolve()),
        "installed managed executable is outside the installed plugin data root",
    )
    staged_launcher = verify_flat_managed_layout(
        args.installed_plugin_data.resolve(),
        manifest["release_version"],
        manifest["asset_target"],
    )
    require(
        same_existing_path(managed_binary, staged_launcher),
        "installed managed executable is not the flat provisioned launcher",
    )
    require(
        managed_binary != cli.resolve(),
        "installed proof used the unpacked package executable as its managed runtime",
    )
    return managed, managed_binary


def _live_retrieval(
    args: argparse.Namespace,
    setup: RuntimeSetup,
    hosts: HostPair,
    cold: ColdProof,
    manifest: dict,
) -> dict | None:
    live_tasks = {}
    # Calibration already proved both projects in the cold phase. Its draft
    # measurements keep the resident process set live through the tiny project
    # instead of starting another full-project activation.
    if args.proof_tier != "calibration":
        live_tasks["packet-a"] = lambda: hosts.host_a.tool_until_ready(
            "packet",
            {
                "project": str(setup.project_a),
                "question": args.question,
                "budget": "compact",
            },
            "packet-a",
        )
    project_b_query = _live_project_b_query(args, setup)
    live_tasks["search-b-live"] = lambda: hosts.host_b.search_until_ready(
        {"project": str(setup.project_b), "query": project_b_query, "why": True},
        "search-b-live",
    )
    run_parallel(live_tasks)
    after = server_snapshot(
        hosts.host_b.engine_diagnostics(setup.project_b, "diagnostics-after-live"),
        manifest,
        require_resident=True,
    )
    require(
        after["engine"]["successful_encode_count"]
        > cold.snapshot_a["engine"]["successful_encode_count"],
        "successful encode counter did not advance across two-host retrieval",
    )
    require(
        after["process"]["server_instance_id"]
        == cold.shared_identity["server_instance_id"],
        "live retrieval replaced the shared server",
    )
    if not args.produce_qualification_evidence:
        return None
    return capture_five_process_memory(
        args=args,
        node_path=setup.node,
        host_a=hosts.host_a,
        host_a_start=hosts.start_a,
        host_b=hosts.host_b,
        host_b_start=hosts.start_b,
        status_a=cold.status_a,
        status_b=cold.status_b,
        snapshot=after,
        manifest=manifest,
        expected_backend=cold.identity_a["embedding_backend"],
    )


def _continuity_proof(
    args: argparse.Namespace,
    setup: RuntimeSetup,
    hosts: HostPair,
    cold: ColdProof,
    manifest: dict,
    out_dir: Path,
) -> ContinuityProof:
    hosts.host_a.kill()
    hosts.host_b.search_until_ready(
        {"project": str(setup.project_b), "query": setup.query_b, "why": True},
        "survivor-search",
    )
    survivor = server_snapshot(
        hosts.host_b.engine_diagnostics(setup.project_b, "survivor-diagnostics"),
        manifest,
        require_resident=True,
    )
    require(
        survivor["process"]["server_instance_id"]
        == cold.shared_identity["server_instance_id"],
        "one client exit disrupted the surviving client or replaced the server",
    )
    # Replacement-host continuity needs the same resident server, not a second
    # activation of the full calibration source tree.
    if args.proof_tier == "calibration":
        rejoin_project = setup.project_b
        rejoin_query = setup.query_b
    else:
        rejoin_project = setup.project_a
        rejoin_query = args.query
    host_c = McpProcess(
        setup.command,
        env=setup.qualified_env,
        cwd=rejoin_project,
        timeout=args.timeout_secs,
    )
    start_c = process_start_identity(host_c.process.pid)
    try:
        require(
            (host_c.process.pid, start_c)
            not in {
                (hosts.host_a.process.pid, hosts.start_a),
                (hosts.host_b.process.pid, hosts.start_b),
            },
            "replacement plugin host was not independently started",
        )
        host_c.initialize()
        host_c.search_until_ready(
            {"project": str(rejoin_project), "query": rejoin_query, "why": True},
            "rejoin-search",
        )
        diagnostics = host_c.engine_diagnostics(rejoin_project, "rejoin-diagnostics")
        rejoin_identity = engine_identity(
            diagnostics,
            args.engine_policy,
            args.expected_backend,
        )
        rejoin_snapshot = server_snapshot(diagnostics, manifest, require_resident=True)
        require(
            rejoin_snapshot["process"]["server_instance_id"]
            == cold.shared_identity["server_instance_id"],
            "new plugin host did not join the existing server",
        )
        return ContinuityProof(survivor, rejoin_snapshot, rejoin_identity)
    finally:
        write_json(
            out_dir / "plugin-host-c-mcp.json",
            retained_mcp_transcript(host_c.transcript),
        )
        host_c.close()


def _materialized_model(setup: RuntimeSetup, cold: ColdProof) -> Path:
    models = list(setup.embedded_models.rglob("*.gguf"))
    require(
        len(models) == 1, "two-host first use did not materialize exactly one model"
    )
    require(
        sha256(models[0]) == cold.identity_a["embedding_model_sha256"],
        "materialized model digest does not match runtime identity",
    )
    return models[0]
