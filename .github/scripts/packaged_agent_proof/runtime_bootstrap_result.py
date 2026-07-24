"""Runtime evidence assembly and retention."""

from __future__ import annotations

import argparse
from pathlib import Path

from .contracts import assert_retained_json_privacy, retained_runtime_evidence, sha256, write_json
from .foundation import LOWER_TIER_NONCLAIMS
from .installation import assert_no_legacy_state
from .process import current_account_identity, opaque_repository_id
from .runtime_bootstrap_types import ColdProof, ContinuityProof, HostPair, RuntimeSetup

def _runtime_result(
    args: argparse.Namespace,
    cli: Path,
    root: Path,
    setup: RuntimeSetup,
    hosts: HostPair,
    cold: ColdProof,
    snippet: dict,
    snippet_attempts: int,
    managed_runtime: dict | None,
    managed_binary: Path | None,
    memory: dict | None,
    continuity: ContinuityProof,
    materialized: Path,
) -> dict:
    qualification_cli = managed_binary if managed_binary is not None else cli.resolve()
    forbidden = [
        str(setup.project_a),
        str(setup.project_b),
        str(setup.plugin_root),
        str(cli.resolve()),
        str(root.resolve()),
        setup.qualified_env["CODESTORY_EMBED_QUALIFICATION_DIR"],
        setup.qualified_env["CODESTORY_EMBED_QUALIFICATION_NONCE"],
        args.query,
        args.question,
        setup.query_b,
    ]
    if managed_binary is not None:
        forbidden.append(str(managed_binary))
    return {
        "proof_tier": args.proof_tier,
        "qualification_control": setup.qualification_control,
        "same_account": {
            "account_id": current_account_identity(),
            "relation": "same_os_account",
            "cross_login_or_terminal_sessions_proven": False,
            "plugin_hosts": [
                {
                    "pid": hosts.host_a.process.pid,
                    "process_start_id": hosts.start_a,
                    "repository_id": opaque_repository_id(setup.project_a),
                },
                {
                    "pid": hosts.host_b.process.pid,
                    "process_start_id": hosts.start_b,
                    "repository_id": opaque_repository_id(setup.project_b),
                },
            ],
        },
        "cold_race_wall_ms": cold.wall_ms,
        "cold_search_attempts": {
            "host_a": cold.results["search-a"][1],
            "host_b": cold.results["search-b"][1],
        },
        "mcp_public_contract": {
            "ground_attempts": cold.ground_attempts,
            "ground_project_bound": True,
            "snippet_scope": snippet["scope"],
            "requested_context": snippet["requested_context"],
            "snippet_attempts": snippet_attempts,
            "named_resource": "snippet",
            "project_bound": True,
        },
        "shared_identity": cold.shared_identity,
        "snapshot_a": cold.snapshot_a,
        "snapshot_b": cold.snapshot_b,
        "survivor_snapshot": continuity.survivor,
        "rejoin_snapshot": continuity.rejoin_snapshot,
        "identity": cold.identity_a,
        "second_host_identity": cold.identity_b,
        "rejoin_identity": continuity.rejoin_identity,
        "materialization": {
            "sha256": sha256(materialized),
            "reused_on_rejoin": continuity.rejoin_identity[
                "embedding_materialized_reused"
            ],
        },
        "installed_plugin": setup.provenance,
        "managed_runtime": managed_runtime,
        "_qualification_cli_path": str(qualification_cli),
        "_qualification_projects": [str(setup.project_a), str(setup.project_b)],
        "_memory_observations": memory,
        "_qualification_forbidden_values": forbidden,
        "nonclaims": {
            claim: {
                "claimed": False,
                "reason": "hosted two-process package evidence does not establish this claim",
            }
            for claim in sorted(LOWER_TIER_NONCLAIMS)
        },
    }


def _retain_public_runtime(
    result: dict,
    setup: RuntimeSetup,
    out_dir: Path,
) -> None:
    assert_no_legacy_state(Path(setup.qualified_env["CODESTORY_CACHE_ROOT"]))
    public_runtime_evidence = out_dir / "two-host-server-proof.json"
    write_json(public_runtime_evidence, retained_runtime_evidence(result))
    forbidden = result.get("_qualification_forbidden_values", [])
    for artifact in (
        out_dir / "plugin-host-a-mcp.json",
        out_dir / "plugin-host-b-mcp.json",
        out_dir / "plugin-host-c-mcp.json",
        public_runtime_evidence,
    ):
        assert_retained_json_privacy(artifact, forbidden)
