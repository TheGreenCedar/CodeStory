"""Cold shared-server and snippet-contract runtime proof."""

from __future__ import annotations

import argparse
import time

from .contract_primitives import require_nonempty_string
from .foundation import project_node_resource_uri, require, resource_uri_matches
from .installation_support import run_parallel
from .runtime_bootstrap_types import ColdProof, HostPair, RuntimeSetup
from .server_cleanup import pin_temporary_package_server
from .server_engine_identity import engine_identity
from .server_identity import assert_public_status, server_snapshot, shared_server_identity


def _cold_shared_proof(
    args: argparse.Namespace,
    setup: RuntimeSetup,
    hosts: HostPair,
    manifest: dict,
    cleanup_control: dict,
) -> ColdProof:
    run_parallel(
        {
            "initialize-a": hosts.host_a.initialize,
            "initialize-b": hosts.host_b.initialize,
        }
    )
    ground_response, ground_attempts = hosts.host_a.tool_until_ready(
        "ground",
        {"project": str(setup.project_a), "budget": "strict"},
        "installed-ground-a",
    )
    ground = ground_response["result"]["structuredContent"]
    require(
        isinstance(ground, dict) and ground,
        f"installed runtime ground returned no structured result: {ground!r}",
    )
    started = time.perf_counter()
    results = run_parallel(
        {
            "search-a": lambda: hosts.host_a.search_until_ready(
                {"project": str(setup.project_a), "query": args.query, "why": True},
                "cold-search-a",
            ),
            "search-b": lambda: hosts.host_b.search_until_ready(
                {"project": str(setup.project_b), "query": setup.query_b, "why": True},
                "cold-search-b",
            ),
        }
    )
    wall_ms = round((time.perf_counter() - started) * 1000, 3)
    diagnostics_a = hosts.host_a.engine_diagnostics(setup.project_a, "diagnostics-a")
    diagnostics_b = hosts.host_b.engine_diagnostics(setup.project_b, "diagnostics-b")
    identity_a = engine_identity(
        diagnostics_a, args.engine_policy, args.expected_backend
    )
    identity_b = engine_identity(
        diagnostics_b, args.engine_policy, args.expected_backend
    )
    snapshot_a = server_snapshot(diagnostics_a, manifest, require_resident=True)
    snapshot_b = server_snapshot(diagnostics_b, manifest, require_resident=True)
    shared_identity = shared_server_identity(snapshot_a, snapshot_b)
    if setup.target_os == "windows":
        pin_temporary_package_server(
            cleanup_control,
            snapshot_a["process"],
            manifest,
            setup.target_os,
            "initial temporary package embedding server",
        )
    require(
        identity_a["embedding_engine_instance_id"]
        == identity_b["embedding_engine_instance_id"],
        "independent plugin hosts observed different engine instances",
    )
    require(
        identity_a["embedding_engine_load_generation"]
        == identity_b["embedding_engine_load_generation"]
        == shared_identity["load_generation"],
        "engine load generation disagrees with server proof",
    )
    require(
        identity_a["embedding_model_load_count"]
        == identity_b["embedding_model_load_count"]
        == shared_identity["model_load_count"]
        == 1,
        "two-host cold race did not prove one model load",
    )
    status_a = hosts.host_a.status(setup.project_a, "status-a")
    status_b = hosts.host_b.status(setup.project_b, "status-b")
    assert_public_status(status_a)
    assert_public_status(status_b)
    return ColdProof(
        results,
        wall_ms,
        ground_attempts,
        identity_a,
        identity_b,
        snapshot_a,
        snapshot_b,
        shared_identity,
        status_a,
        status_b,
    )


def _snippet_contract(
    setup: RuntimeSetup, hosts: HostPair, cold: ColdProof
) -> tuple[dict, int]:
    search = cold.results["search-b"][0]["result"]["structuredContent"]
    linked_hit = next(
        (
            hit
            for hit in search["hits"]
            if isinstance(hit, dict)
            and isinstance(hit.get("node_id"), str)
            and isinstance(hit.get("links"), list)
        ),
        None,
    )
    require(
        isinstance(linked_hit, dict),
        f"packaged search omitted a resolvable hit with continuation links: {search!r}",
    )
    linked_node_id = linked_hit["node_id"]
    expected_uri = project_node_resource_uri(
        "codestory://snippet",
        linked_node_id,
        setup.project_b,
    )
    linked_uri = next(
        (
            link.get("uri")
            for link in linked_hit["links"]
            if isinstance(link, dict) and link.get("rel") == "snippet"
        ),
        None,
    )
    require(
        isinstance(linked_uri, str) and resource_uri_matches(expected_uri, linked_uri),
        "packaged search returned a missing or noncanonical project-bound snippet link",
    )
    resource_node = hosts.host_b.resource(linked_uri, "snippet-resource-contract").get(
        "node"
    )
    require(
        isinstance(resource_node, dict) and resource_node.get("id") == linked_node_id,
        "project-bound snippet resource returned a different node",
    )
    response, attempts = hosts.host_b.tool_until_ready(
        "snippet",
        {
            "project": str(setup.project_b),
            "id": linked_node_id,
            "function_body": True,
            "lines": 0,
        },
        "snippet-contract",
    )
    snippet = response["result"]["structuredContent"]
    require(
        snippet.get("scope") == "function_body",
        f"packaged snippet ignored function_body selection: {snippet!r}",
    )
    require(
        snippet.get("requested_context") == 0,
        f"packaged snippet ignored the bounded lines alias: {snippet!r}",
    )
    require_nonempty_string(
        snippet.get("range_source"), "packaged function-body snippet range source"
    )
    require(
        setup.query_b in snippet.get("snippet", ""),
        f"packaged function-body snippet omitted the selected symbol: {snippet!r}",
    )
    node = snippet.get("node")
    require(
        isinstance(node, dict),
        f"packaged function-body snippet omitted node identity: {snippet!r}",
    )
    node_id = require_nonempty_string(
        node.get("id"),
        "packaged function-body snippet node id",
    )
    require(
        node_id == linked_node_id,
        "function-body snippet changed the exact linked node identity",
    )
    return snippet, attempts
