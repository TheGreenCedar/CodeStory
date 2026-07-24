"""Live packaged runtime proof orchestration."""

from __future__ import annotations

import argparse
import hashlib
import shutil
import time
from dataclasses import dataclass
from pathlib import Path

from .contracts import (
    assert_retained_json_privacy,
    require_nonempty_string,
    retained_mcp_transcript,
    retained_runtime_evidence,
    sha256,
    write_json,
    write_private_json,
)
from .foundation import (
    LOWER_TIER_NONCLAIMS,
    TARGET_CONTRACTS,
    project_node_resource_uri,
    require,
    resource_uri_matches,
)
from .installation import (
    assert_no_legacy_state,
    create_second_repository,
    installed_plugin_identity,
    qualification_environment,
    run_parallel,
    verify_managed_runtime_status,
)
from .process import (
    McpProcess,
    assert_public_status,
    capture_five_process_memory,
    current_account_identity,
    engine_identity,
    opaque_repository_id,
    pin_temporary_package_server,
    process_start_identity,
    server_snapshot,
    shared_server_identity,
)


@dataclass(frozen=True)
class RuntimeSetup:
    project_a: Path
    project_b: Path
    query_b: str
    plugin_root: Path
    provenance: dict | None
    node: Path
    qualified_env: dict[str, str]
    qualification_control: dict
    target_os: str
    command: list[str]
    embedded_models: Path


@dataclass(frozen=True)
class HostPair:
    host_a: McpProcess
    host_b: McpProcess
    start_a: str
    start_b: str


@dataclass(frozen=True)
class ColdProof:
    results: dict
    wall_ms: float
    ground_attempts: int
    identity_a: dict
    identity_b: dict
    snapshot_a: dict
    snapshot_b: dict
    shared_identity: dict
    status_a: dict
    status_b: dict


@dataclass(frozen=True)
class ContinuityProof:
    survivor: dict
    rejoin_snapshot: dict
    rejoin_identity: dict


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
    identity_a = engine_identity(diagnostics_a, args.engine_policy, args.expected_backend)
    identity_b = engine_identity(diagnostics_b, args.engine_policy, args.expected_backend)
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


def _snippet_contract(setup: RuntimeSetup, hosts: HostPair, cold: ColdProof) -> tuple[dict, int]:
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
    resource_node = hosts.host_b.resource(linked_uri, "snippet-resource-contract").get("node")
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
    require_nonempty_string(snippet.get("range_source"), "packaged function-body snippet range source")
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
        after["process"]["server_instance_id"] == cold.shared_identity["server_instance_id"],
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
        survivor["process"]["server_instance_id"] == cold.shared_identity["server_instance_id"],
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
    require(len(models) == 1, "two-host first use did not materialize exactly one model")
    require(
        sha256(models[0]) == cold.identity_a["embedding_model_sha256"],
        "materialized model digest does not match runtime identity",
    )
    return models[0]


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
            cold,
            snippet,
            snippet_attempts,
            managed_runtime,
            managed_binary,
            memory,
            continuity,
            materialized,
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
