"""Managed-runtime, live retrieval, continuity, and model proof phases."""

from __future__ import annotations

import argparse
from pathlib import Path

from .contracts import (
    require_nonempty_string,
    retained_mcp_transcript,
    sha256,
    write_json,
)
from .foundation import require
from .installation import run_parallel, verify_managed_runtime_status
from .process import (
    McpProcess,
    capture_five_process_memory,
    engine_identity,
    process_start_identity,
    server_snapshot,
)
from .runtime_bootstrap_types import ColdProof, ContinuityProof, HostPair, RuntimeSetup


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
    run_parallel(
        {
            "packet-a": lambda: hosts.host_a.tool_until_ready(
                "packet",
                {
                    "project": str(setup.project_a),
                    "question": args.question,
                    "budget": "compact",
                },
                "packet-a",
            ),
            "search-b-live": lambda: hosts.host_b.search_until_ready(
                {"project": str(setup.project_b), "query": setup.query_b, "why": True},
                "search-b-live",
            ),
        }
    )
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
    host_c = McpProcess(
        setup.command,
        env=setup.qualified_env,
        cwd=setup.project_a,
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
            {"project": str(setup.project_a), "query": args.query, "why": True},
            "rejoin-search",
        )
        diagnostics = host_c.engine_diagnostics(setup.project_a, "rejoin-diagnostics")
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
