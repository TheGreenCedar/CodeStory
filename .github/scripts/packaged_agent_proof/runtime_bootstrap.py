"""Live packaged runtime proof orchestration."""

from __future__ import annotations

import argparse
from pathlib import Path

from .contracts import retained_mcp_transcript, write_json
from .runtime_bootstrap_cold import _cold_shared_proof, _snippet_contract
from .runtime_bootstrap_continuity import (
    _continuity_proof,
    _live_retrieval,
    _managed_runtime,
    _materialized_model,
)
from .runtime_bootstrap_result import _retain_public_runtime, _runtime_result
from .runtime_bootstrap_setup import _runtime_setup, _start_hosts
from .runtime_bootstrap_types import RuntimePhaseEvidence


def prove_runtime(
    args: argparse.Namespace,
    cli: Path,
    env: dict[str, str],
    root: Path,
    out_dir: Path,
    manifest: dict,
    server_cleanup_control: dict,
) -> dict:
    setup = _runtime_setup(args, cli, env, root, manifest, server_cleanup_control)
    hosts = _start_hosts(args, setup)
    try:
        cold = _cold_shared_proof(
            args,
            setup,
            hosts,
            manifest,
            server_cleanup_control,
        )
        snippet, snippet_attempts = _snippet_contract(setup, hosts, cold)
        managed_runtime, managed_binary = _managed_runtime(
            args,
            cli,
            setup,
            cold,
            manifest,
        )
        memory = _live_retrieval(args, setup, hosts, cold, manifest)
        continuity = _continuity_proof(args, setup, hosts, cold, manifest, out_dir)
        materialized = _materialized_model(setup, cold)
        result = _runtime_result(
            args,
            cli,
            root,
            setup,
            hosts,
            RuntimePhaseEvidence(
                cold=cold,
                snippet=snippet,
                snippet_attempts=snippet_attempts,
                managed_runtime=managed_runtime,
                managed_binary=managed_binary,
                memory=memory,
                continuity=continuity,
                materialized_model=materialized,
            ),
        )
    finally:
        write_json(
            out_dir / "plugin-host-a-mcp.json",
            retained_mcp_transcript(hosts.host_a.transcript),
        )
        write_json(
            out_dir / "plugin-host-b-mcp.json",
            retained_mcp_transcript(hosts.host_b.transcript),
        )
        hosts.host_a.close()
        hosts.host_b.close()
    _retain_public_runtime(result, setup, out_dir)
    return result
