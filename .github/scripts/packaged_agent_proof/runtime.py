"""Runtime for packaged CodeStory proof."""

from .foundation import *
from .contracts import (
    ProofFailure,
    assert_retained_json_privacy,
    canonical_sha256,
    load_holdout_task_contracts,
    qualification_measurement_sample_value,
    require,
    require_exact_keys,
    require_nonempty_string,
    require_nonnegative_int,
    require_opaque_identifier,
    require_positive_int,
    require_sha256,
    retained_mcp_transcript,
    retained_runtime_evidence,
    selected_qualification_matrix_cell,
    sha256,
    write_json,
    write_private_json,
)
from .process import (
    McpProcess,
    assert_public_status,
    capture_five_process_memory,
    current_account_identity,
    engine_identity,
    json_command,
    opaque_repository_id,
    pin_temporary_package_server,
    process_start_identity,
    run,
    server_snapshot,
    shared_server_identity,
)
from .installation import (
    assert_no_legacy_state,
    create_second_repository,
    installed_plugin_provenance,
    qualification_environment,
    run_parallel,
    verify_managed_runtime_status,
)

def retain_five_process_memory_evidence(
    artifact_root: Path,
    raw: object,
    *,
    source: dict,
    package: dict,
    contracts: dict,
    protocol: dict,
    target: str,
    proof_tier: str,
    matrix_cell_id: str,
    expected_policy: str,
    expected_backend: str,
    forbidden_values: list[str],
) -> dict:
    require(isinstance(raw, dict), "live runtime omitted five-process memory observations")
    require_exact_keys(
        raw,
        {"evidence_contract", "metric", "unit", "samples"},
        "five-process memory observations",
    )
    payload = {
        "schema_version": 1,
        "source": source,
        "package": package,
        "contracts": contracts,
        **raw,
    }
    name = "total-codestory-process-memory.raw.json"
    path = artifact_root / name
    write_private_json(path, payload)
    payload_bytes = path.read_bytes()
    for forbidden in forbidden_values:
        require(
            forbidden.encode("utf-8") not in payload_bytes,
            "five-process memory artifact leaked private request material",
        )
    require_exact_keys(
        payload,
        {
            "schema_version",
            "source",
            "package",
            "contracts",
            "evidence_contract",
            "metric",
            "unit",
            "samples",
        },
        "five-process memory artifact",
    )
    require(
        payload["schema_version"] == 1
        and payload["source"] == source
        and payload["package"] == package
        and payload["contracts"] == contracts
        and payload["evidence_contract"] == MEMORY_EVIDENCE_CONTRACT
        and payload["metric"] == "total_codestory_process_memory"
        and payload["unit"] == "bytes",
        "five-process memory artifact changed its bound contract",
    )
    matrix_cell = selected_qualification_matrix_cell(
        protocol,
        cell_id=matrix_cell_id,
        target=target,
        proof_tier=proof_tier,
        expected_policy=expected_policy,
        expected_backend=expected_backend,
    )
    samples = payload["samples"]
    require(
        isinstance(samples, list) and len(samples) == 3,
        "five-process memory evidence requires three samples",
    )
    target_os = TARGET_CONTRACTS[target]["target_os"]
    clock_policy = protocol["clock_policy"]
    allowed_awake_apis = set(clock_policy["platform_apis"][target_os])
    suspend_contract = clock_policy["suspend_detection"]
    expected_measurement_api = {
        "linux": "rss",
        "macos": "macos_physical_footprint",
        "windows": "windows_working_set",
    }[target_os]
    values = []
    sample_ids: set[str] = set()
    server_identities: set[tuple[str, str, int]] = set()
    for index, sample in enumerate(samples):
        require(isinstance(sample, dict), f"five-process memory sample {index} is malformed")
        require_exact_keys(
            sample,
            {
                "sample_id",
                "repeat",
                "matrix_cell_id",
                "workload_id",
                "cache_state",
                "residency_state",
                "producer_process",
                "server_identity",
                "clock",
                "start",
                "end",
                "operands",
                "suspend_witness",
            },
            f"five-process memory sample {index}",
        )
        sample_id = require_opaque_identifier(
            sample["sample_id"],
            f"five-process memory sample {index}.sample_id",
        )
        require(sample_id not in sample_ids, "five-process memory sample id was reused")
        sample_ids.add(sample_id)
        require(
            sample["repeat"] == index + 1
            and sample["matrix_cell_id"] == matrix_cell_id
            and sample["workload_id"]
            == protocol["workloads"]["total_codestory_process_memory"]["workload_id"]
            and sample["cache_state"] == matrix_cell["cache_state"]
            and sample["residency_state"] == matrix_cell["residency_state"],
            "five-process memory sample changed its preregistered cell or workload",
        )
        processes = sample.get("operands", {}).get("processes", [])
        require(
            all(
                isinstance(process, dict)
                and process.get("measurement_api") == expected_measurement_api
                for process in processes
            ),
            "five-process memory sample used the wrong platform memory API",
        )
        package_processes = {
            "plugin_cli_a",
            "plugin_cli_b",
            "embedding_server",
        }
        require(
            all(
                process.get("executable_sha256") == package["executable_sha256"]
                for process in processes
                if process.get("role") in package_processes
            ),
            "five-process memory sample used a different packaged executable",
        )
        server = sample["server_identity"]
        require(
            isinstance(server, dict),
            f"five-process memory sample {index} server identity is malformed",
        )
        require_exact_keys(
            server,
            {"server_instance_id", "process_start_id", "load_generation"},
            f"five-process memory sample {index} server identity",
        )
        server_identities.add(
            (
                require_opaque_identifier(
                    server["server_instance_id"],
                    f"five-process memory sample {index}.server_instance_id",
                ),
                require_nonempty_string(
                    server["process_start_id"],
                    f"five-process memory sample {index}.server_process_start_id",
                ),
                require_positive_int(
                    server["load_generation"],
                    f"five-process memory sample {index}.load_generation",
                ),
            )
        )
        adapted = {**sample, "process": sample["producer_process"]}
        adapted.pop("producer_process")
        values.append(
            qualification_measurement_sample_value(
                "total_codestory_process_memory",
                adapted,
                contracts=contracts,
                phase_boundaries=protocol["phase_boundaries"],
                allowed_awake_apis=allowed_awake_apis,
                inclusive_api=suspend_contract["platform_apis"][target_os],
                maximum_suspend_ns=suspend_contract[
                    "maximum_inclusive_minus_awake_ns"
                ],
                expected_policy=expected_policy,
                expected_backend=expected_backend,
            )
        )
    require(
        len(server_identities) == 1,
        "five-process memory block changed shared server identity",
    )
    return {
        "artifact": {
            "name": name,
            "sha256": hashlib.sha256(payload_bytes).hexdigest(),
        },
        "value": max(values),
        "payload": payload,
    }


def prove_runtime(
    args: argparse.Namespace,
    cli: Path,
    env: dict[str, str],
    root: Path,
    out_dir: Path,
    manifest: dict,
    server_cleanup_control: dict,
) -> dict:
    require(args.plugin_handoff, "runtime proof requires the ordinary packaged plugin handoff")
    require(args.plugin_root is not None, "--plugin-handoff requires --plugin-root")
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

    plugin_root = args.plugin_root.resolve()
    provenance = (
        installed_plugin_provenance(args, plugin_root, manifest)
        if args.proof_tier == "installed_runtime"
        else None
    )
    launcher = plugin_root / "scripts" / "codestory-mcp.cjs"
    require(launcher.is_file(), f"plugin launcher is missing: {launcher}")
    node = shutil.which("node")
    require(node is not None, "packaged plugin proof requires Node.js for the host launcher")
    qualified_env, qualification_control = qualification_environment(root, env)
    qualified_env.pop("CODESTORY_CLI", None)
    target_os = TARGET_CONTRACTS[manifest["asset_target"]]["target_os"]
    if args.proof_tier == "installed_runtime":
        qualified_env["CODESTORY_PLUGIN_DATA"] = str(args.installed_plugin_data.resolve())
        if args.installed_plugin_source == "candidate":
            candidate_archive_sha256 = sha256(args.archive)
            qualified_env[
                "CODESTORY_PLUGIN_CANDIDATE_ARCHIVE_SHA256"
            ] = candidate_archive_sha256
            write_private_json(
                Path(qualified_env["CODESTORY_EMBED_QUALIFICATION_DIR"])
                / "candidate-managed-install.json",
                {
                    "schema_version": 1,
                    "purpose": "codestory-candidate-managed-install",
                    "archive_sha256": candidate_archive_sha256,
                    "qualification_nonce_sha256": hashlib.sha256(
                        qualified_env[
                            "CODESTORY_EMBED_QUALIFICATION_NONCE"
                        ].encode("ascii")
                    ).hexdigest(),
                },
            )
    else:
        qualified_env["CODESTORY_CLI"] = str(cli)
    command = [node, str(launcher)]
    server_cleanup_control.update(
        {
            "qualification_cli": str(cli.resolve()),
            "qualification_directory": qualified_env[
                "CODESTORY_EMBED_QUALIFICATION_DIR"
            ],
            "qualification_nonce": qualified_env[
                "CODESTORY_EMBED_QUALIFICATION_NONCE"
            ],
            "plugin_cli_archive_sha256": None,
            "projects": [str(project_a), str(project_b)],
        }
    )

    embedded_models = Path(qualified_env["CODESTORY_CACHE_ROOT"]) / "embedded-models"
    require(not embedded_models.exists(), "isolated proof cache was not empty before first use")
    host_a = McpProcess(command, env=qualified_env, cwd=project_a, timeout=args.timeout_secs)
    host_b = McpProcess(command, env=qualified_env, cwd=project_b, timeout=args.timeout_secs)
    host_a_start = process_start_identity(host_a.process.pid)
    host_b_start = process_start_identity(host_b.process.pid)
    require(
        (host_a.process.pid, host_a_start) != (host_b.process.pid, host_b_start),
        "plugin hosts are not independent processes",
    )
    try:
        run_parallel({"initialize-a": host_a.initialize, "initialize-b": host_b.initialize})
        ground_response, ground_attempts = host_a.tool_until_ready(
            "ground",
            {
                "project": str(project_a),
                "budget": "strict",
            },
            "installed-ground-a",
        )
        ground = ground_response["result"]["structuredContent"]
        require(
            isinstance(ground, dict) and ground,
            f"installed runtime ground returned no structured result: {ground!r}",
        )
        cold_started = time.perf_counter()
        cold_results = run_parallel(
            {
                "search-a": lambda: host_a.search_until_ready(
                    {"project": str(project_a), "query": args.query, "why": True},
                    "cold-search-a",
                ),
                "search-b": lambda: host_b.search_until_ready(
                    {"project": str(project_b), "query": query_b, "why": True},
                    "cold-search-b",
                ),
            }
        )
        cold_race_wall_ms = round((time.perf_counter() - cold_started) * 1000, 3)
        diagnostics_a = host_a.engine_diagnostics(project_a, "diagnostics-a")
        diagnostics_b = host_b.engine_diagnostics(project_b, "diagnostics-b")
        identity_a = engine_identity(
            diagnostics_a,
            args.engine_policy,
            args.expected_backend,
        )
        identity_b = engine_identity(
            diagnostics_b,
            args.engine_policy,
            args.expected_backend,
        )
        snapshot_a = server_snapshot(diagnostics_a, manifest, require_resident=True)
        snapshot_b = server_snapshot(diagnostics_b, manifest, require_resident=True)
        shared_identity = shared_server_identity(snapshot_a, snapshot_b)
        if target_os == "windows":
            pin_temporary_package_server(
                server_cleanup_control,
                snapshot_a["process"],
                manifest,
                target_os,
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
        status_a = host_a.status(project_a, "status-a")
        status_b = host_b.status(project_b, "status-b")
        assert_public_status(status_a)
        assert_public_status(status_b)
        search_b = cold_results["search-b"][0]["result"]["structuredContent"]
        search_b_hits = search_b["hits"]
        linked_hit = next(
            (
                hit
                for hit in search_b_hits
                if isinstance(hit, dict)
                and isinstance(hit.get("node_id"), str)
                and isinstance(hit.get("links"), list)
            ),
            None,
        )
        require(
            isinstance(linked_hit, dict),
            f"packaged search omitted a resolvable hit with continuation links: {search_b!r}",
        )
        linked_node_id = linked_hit["node_id"]
        expected_snippet_uri = project_node_resource_uri(
            "codestory://snippet",
            linked_node_id,
            project_b,
        )
        linked_snippet_uri = next(
            (
                link.get("uri")
                for link in linked_hit["links"]
                if isinstance(link, dict) and link.get("rel") == "snippet"
            ),
            None,
        )
        require(
            isinstance(linked_snippet_uri, str)
            and resource_uri_matches(expected_snippet_uri, linked_snippet_uri),
            "packaged search returned a missing or noncanonical project-bound snippet link",
        )
        snippet_resource = host_b.resource(
            linked_snippet_uri,
            "snippet-resource-contract",
        )
        snippet_resource_node = snippet_resource.get("node")
        require(
            isinstance(snippet_resource_node, dict)
            and snippet_resource_node.get("id") == linked_node_id,
            "project-bound snippet resource returned a different node",
        )
        snippet_response, snippet_attempts = host_b.tool_until_ready(
            "snippet",
            {
                "project": str(project_b),
                "id": linked_node_id,
                "function_body": True,
                "lines": 0,
            },
            "snippet-contract",
        )
        snippet = snippet_response["result"]["structuredContent"]
        require(
            snippet.get("scope") == "function_body",
            f"packaged snippet ignored function_body selection: {snippet!r}",
        )
        require(
            snippet.get("requested_context") == 0,
            f"packaged snippet ignored the bounded lines alias: {snippet!r}",
        )
        require_nonempty_string(
            snippet.get("range_source"),
            "packaged function-body snippet range source",
        )
        require(
            query_b in snippet.get("snippet", ""),
            f"packaged function-body snippet omitted the selected symbol: {snippet!r}",
        )
        snippet_node = snippet.get("node")
        require(
            isinstance(snippet_node, dict),
            f"packaged function-body snippet omitted node identity: {snippet!r}",
        )
        snippet_node_id = require_nonempty_string(
            snippet_node.get("id"),
            "packaged function-body snippet node id",
        )
        require(
            snippet_node_id == linked_node_id,
            "function-body snippet changed the exact linked node identity",
        )
        managed_runtime = None
        if args.proof_tier == "installed_runtime":
            managed_runtime = verify_managed_runtime_status(
                status_a,
                plugin_root=plugin_root,
                manifest=manifest,
                archive_sha256=sha256(args.archive),
            )
            require(
                verify_managed_runtime_status(
                    status_b,
                    plugin_root=plugin_root,
                    manifest=manifest,
                    archive_sha256=sha256(args.archive),
                )
                == managed_runtime,
                "independent installed plugin hosts reported different managed runtime provenance",
            )
            if args.installed_plugin_source == "candidate":
                require(
                    managed_runtime["build_source"] == "candidate_archive"
                    and managed_runtime["repo_ref"]
                    == manifest["source"]["commit"],
                    "candidate installed proof did not launch the staged candidate archive",
                )
            else:
                require(
                    managed_runtime["build_source"] == "github_release"
                    and managed_runtime["repo_ref"]
                    == f"v{manifest['release_version']}",
                    "marketplace installed proof did not launch the published release archive",
                )
            managed_binary_path = Path(
                require_nonempty_string(
                    status_a["plugin_runtime"].get("managed_binary_path"),
                    "installed plugin_runtime.managed_binary_path",
                )
            ).resolve()
            require(
                managed_binary_path.is_relative_to(args.installed_plugin_data.resolve()),
                "installed managed executable is outside the installed plugin data root",
            )
            require(
                managed_binary_path != cli.resolve(),
                "installed proof used the unpacked package executable as its managed runtime",
            )

        before_encode = snapshot_a["engine"]["successful_encode_count"]
        run_parallel(
            {
                "packet-a": lambda: host_a.tool_until_ready(
                    "packet",
                    {
                        "project": str(project_a),
                        "question": args.question,
                        "budget": "compact",
                    },
                    "packet-a",
                ),
                "search-b-live": lambda: host_b.search_until_ready(
                    {"project": str(project_b), "query": query_b, "why": True},
                    "search-b-live",
                ),
            }
        )
        after_diagnostics = host_b.engine_diagnostics(project_b, "diagnostics-after-live")
        after_snapshot = server_snapshot(after_diagnostics, manifest, require_resident=True)
        require(
            after_snapshot["engine"]["successful_encode_count"] > before_encode,
            "successful encode counter did not advance across two-host retrieval",
        )
        require(
            after_snapshot["process"]["server_instance_id"]
            == shared_identity["server_instance_id"],
            "live retrieval replaced the shared server",
        )
        memory_observations = (
            capture_five_process_memory(
                args=args,
                node_path=Path(node),
                host_a=host_a,
                host_a_start=host_a_start,
                host_b=host_b,
                host_b_start=host_b_start,
                status_a=status_a,
                status_b=status_b,
                snapshot=after_snapshot,
                manifest=manifest,
                expected_backend=identity_a["embedding_backend"],
            )
            if args.produce_qualification_evidence
            else None
        )

        host_a.kill()
        host_b.search_until_ready(
            {"project": str(project_b), "query": query_b, "why": True},
            "survivor-search",
        )
        survivor = server_snapshot(
            host_b.engine_diagnostics(project_b, "survivor-diagnostics"),
            manifest,
            require_resident=True,
        )
        require(
            survivor["process"]["server_instance_id"] == shared_identity["server_instance_id"],
            "one client exit disrupted the surviving client or replaced the server",
        )

        host_c = McpProcess(command, env=qualified_env, cwd=project_a, timeout=args.timeout_secs)
        host_c_start = process_start_identity(host_c.process.pid)
        try:
            require(
                (host_c.process.pid, host_c_start)
                not in {
                    (host_a.process.pid, host_a_start),
                    (host_b.process.pid, host_b_start),
                },
                "replacement plugin host was not independently started",
            )
            host_c.initialize()
            host_c.search_until_ready(
                {"project": str(project_a), "query": args.query, "why": True},
                "rejoin-search",
            )
            rejoin_diagnostics = host_c.engine_diagnostics(project_a, "rejoin-diagnostics")
            rejoin_identity = engine_identity(
                rejoin_diagnostics,
                args.engine_policy,
                args.expected_backend,
            )
            rejoin_snapshot = server_snapshot(
                rejoin_diagnostics,
                manifest,
                require_resident=True,
            )
            require(
                rejoin_snapshot["process"]["server_instance_id"]
                == shared_identity["server_instance_id"],
                "new plugin host did not join the existing server",
            )
        finally:
            write_json(
                out_dir / "plugin-host-c-mcp.json",
                retained_mcp_transcript(host_c.transcript),
            )
            host_c.close()

        cold_models = list(embedded_models.rglob("*.gguf"))
        require(len(cold_models) == 1, "two-host first use did not materialize exactly one model")
        materialized = cold_models[0]
        require(
            sha256(materialized) == identity_a["embedding_model_sha256"],
            "materialized model digest does not match runtime identity",
        )
        result = {
            "proof_tier": args.proof_tier,
            "qualification_control": qualification_control,
            "same_account": {
                "account_id": current_account_identity(),
                "relation": "same_os_account",
                "cross_login_or_terminal_sessions_proven": False,
                "plugin_hosts": [
                    {
                        "pid": host_a.process.pid,
                        "process_start_id": host_a_start,
                        "repository_id": opaque_repository_id(project_a),
                    },
                    {
                        "pid": host_b.process.pid,
                        "process_start_id": host_b_start,
                        "repository_id": opaque_repository_id(project_b),
                    },
                ],
            },
            "cold_race_wall_ms": cold_race_wall_ms,
            "cold_search_attempts": {
                "host_a": cold_results["search-a"][1],
                "host_b": cold_results["search-b"][1],
            },
            "mcp_public_contract": {
                "ground_attempts": ground_attempts,
                "ground_project_bound": True,
                "snippet_scope": snippet["scope"],
                "requested_context": snippet["requested_context"],
                "snippet_attempts": snippet_attempts,
                "named_resource": "snippet",
                "project_bound": True,
            },
            "shared_identity": shared_identity,
            "snapshot_a": snapshot_a,
            "snapshot_b": snapshot_b,
            "survivor_snapshot": survivor,
            "rejoin_snapshot": rejoin_snapshot,
            "identity": identity_a,
            "second_host_identity": identity_b,
            "rejoin_identity": rejoin_identity,
            "materialization": {
                "sha256": sha256(materialized),
                "reused_on_rejoin": rejoin_identity["embedding_materialized_reused"],
            },
            "installed_plugin": provenance,
            "managed_runtime": managed_runtime,
            "_qualification_cli_path": (
                str(managed_binary_path)
                if args.proof_tier == "installed_runtime"
                else str(cli.resolve())
            ),
            "_qualification_projects": [str(project_a), str(project_b)],
            "_memory_observations": memory_observations,
            "_qualification_forbidden_values": [
                str(project_a),
                str(project_b),
                str(plugin_root),
                str(cli.resolve()),
                str(root.resolve()),
                qualified_env["CODESTORY_EMBED_QUALIFICATION_DIR"],
                qualified_env["CODESTORY_EMBED_QUALIFICATION_NONCE"],
                args.query,
                args.question,
                query_b,
                *(
                    [str(managed_binary_path)]
                    if args.proof_tier == "installed_runtime"
                    else []
                ),
            ],
            "nonclaims": {
                claim: {
                    "claimed": False,
                    "reason": "hosted two-process package evidence does not establish this claim",
                }
                for claim in sorted(LOWER_TIER_NONCLAIMS)
            },
        }
    finally:
        write_json(
            out_dir / "plugin-host-a-mcp.json",
            retained_mcp_transcript(host_a.transcript),
        )
        write_json(
            out_dir / "plugin-host-b-mcp.json",
            retained_mcp_transcript(host_b.transcript),
        )
        host_a.close()
        host_b.close()
    assert_no_legacy_state(Path(qualified_env["CODESTORY_CACHE_ROOT"]))
    public_runtime_evidence = out_dir / "two-host-server-proof.json"
    write_json(public_runtime_evidence, retained_runtime_evidence(result))
    forbidden_runtime_values = result.get("_qualification_forbidden_values", [])
    for public_artifact in (
        out_dir / "plugin-host-a-mcp.json",
        out_dir / "plugin-host-b-mcp.json",
        out_dir / "plugin-host-c-mcp.json",
        public_runtime_evidence,
    ):
        assert_retained_json_privacy(public_artifact, forbidden_runtime_values)
    return result


def metric_passes(value: int | float, threshold: int | float, comparison: str) -> bool:
    return {
        "equal": value == threshold,
        "greater_than_or_equal": value >= threshold,
        "less_than_or_equal": value <= threshold,
    }[comparison]


def read_jsonl(path: Path) -> list[dict]:
    if not path.is_file() or path.is_symlink():
        return []
    events = []
    for line in path.read_text(encoding="utf-8").splitlines():
        if not line.strip():
            continue
        try:
            event = json.loads(line)
        except json.JSONDecodeError:
            continue
        if isinstance(event, dict):
            events.append(event)
    return events


def wait_for_jsonl_event(
    path: Path,
    predicate,
    *,
    timeout: int,
    process: subprocess.Popen | None = None,
) -> dict:
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        for event in read_jsonl(path):
            if predicate(event):
                return event
        if process is not None and process.poll() is not None:
            stdout, stderr = process.communicate()
            raise ProofFailure(
                "qualification product process exited before its raw event: "
                f"exit={process.returncode} stdout_sha256="
                f"{hashlib.sha256(stdout.encode('utf-8')).hexdigest()} stderr_sha256="
                f"{hashlib.sha256(stderr.encode('utf-8')).hexdigest()}"
            )
        time.sleep(0.01)
    raise ProofFailure(f"timed out waiting for qualification event file {path.name}")


def send_server_qualification_control(
    directory: Path,
    nonce: str,
    *,
    sequence: int,
    action: str,
    timeout: int,
) -> dict:
    nonce_sha256 = hashlib.sha256(nonce.encode("ascii")).hexdigest()
    command_path = directory / f"{nonce}.command.json"
    require(not command_path.exists(), "stale embedding qualification command is present")
    write_private_json(
        command_path,
        {
            "schema_version": 1,
            "sequence": sequence,
            "nonce_sha256": nonce_sha256,
            "action": action,
            "parameters": {"class": None},
        },
    )
    try:
        event_path = directory / f"{nonce}.events.jsonl"
        event = wait_for_jsonl_event(
            event_path,
            lambda candidate: candidate.get("sequence") == sequence
            and candidate.get("action") == action,
            timeout=timeout,
        )
        require(
            event.get("status") in {"completed", "accepted"},
            f"embedding qualification control {action} failed",
        )
        return event
    finally:
        command_path.unlink(missing_ok=True)


def server_observation_from_control_event(event: dict, phase: str) -> dict:
    snapshot = event.get("snapshot")
    require(isinstance(snapshot, dict), f"{phase} control event omitted its server snapshot")
    process = snapshot.get("process")
    engine = snapshot.get("engine")
    require(isinstance(process, dict), f"{phase} server snapshot omitted process identity")
    require(isinstance(engine, dict), f"{phase} server snapshot omitted resident engine identity")
    process_start = require_nonempty_string(
        process.get("process_start_id"), f"{phase} process start"
    )
    return {
        "phase": phase,
        "server_instance_id": require_opaque_identifier(
            process.get("server_instance_id"), f"{phase} server instance"
        ),
        "process_start_id": hashlib.sha256(process_start.encode("utf-8")).hexdigest(),
        "load_generation": require_positive_int(
            engine.get("load_generation"), f"{phase} load generation"
        ),
    }


def publication_identity_from_status(status: dict) -> str:
    require(status.get("retrieval_mode") == "full", "qualification status is not full")
    contract = status.get("manifest_contract")
    manifest = status.get("manifest")
    require(
        isinstance(contract, dict),
        "qualification status omitted its manifest contract",
    )
    require(
        isinstance(manifest, dict),
        "qualification status omitted its published manifest",
    )
    generation = require_nonempty_string(
        contract.get("generation"), "qualification manifest contract generation"
    )
    input_hash = require_sha256(
        contract.get("input_hash"), "qualification manifest contract input hash"
    )
    project_id = require_opaque_identifier(
        contract.get("project_id"), "qualification manifest contract project"
    )
    schema_version = require_positive_int(
        contract.get("schema_version"), "qualification manifest contract schema"
    )
    graph_hash = require_sha256(
        contract.get("graph_hash"), "qualification manifest contract graph hash"
    )
    require(
        manifest.get("project_id") == project_id
        and manifest.get("sidecar_generation") == generation
        and manifest.get("sidecar_input_hash") == input_hash
        and manifest.get("sidecar_schema_version") == schema_version
        and manifest.get("graph_artifact_hash") == graph_hash,
        "qualification manifest report disagrees with its manifest contract",
    )
    return canonical_sha256(
        {
            "project_id": project_id,
            "generation": generation,
            "input_hash": input_hash,
            "schema_version": schema_version,
            "graph_hash": graph_hash,
            "lexical_version": require_nonempty_string(
                manifest.get("lexical_version"),
                "qualification manifest lexical version",
            ),
            "semantic_generation": require_nonempty_string(
                manifest.get("semantic_generation"),
                "qualification manifest semantic generation",
            ),
            "scip_revision": require_nonempty_string(
                manifest.get("scip_revision"),
                "qualification manifest SCIP revision",
            ),
        }
    )


def run_quality_search(
    cli: Path,
    env: dict[str, str],
    project: Path,
    run_id: str,
    query: str,
    expected: str,
    *,
    timeout: int,
) -> tuple[int | None, str]:
    result, payload = json_command(
        [
            str(cli),
            "search",
            "--project",
            str(project),
            "--query",
            query,
            "--limit",
            "10",
            "--repo-text",
            "off",
            "--refresh",
            "none",
            "--profile",
            "agent",
            "--run-id",
            run_id,
            "--format",
            "json",
        ],
        env=env,
        cwd=project,
        timeout=timeout,
    )
    hits = payload.get("indexed_symbol_hits")
    require(isinstance(hits, list), "qualification search omitted indexed symbol hits")
    position = next(
        (
            index
            for index, hit in enumerate(hits)
            if isinstance(hit, dict)
            and isinstance(hit.get("display_name"), str)
            and expected in hit["display_name"]
        ),
        None,
    )
    rank = None if position is None or position >= 10 else position + 1
    output_sha256 = hashlib.sha256(result["stdout"].encode("utf-8")).hexdigest()
    return rank, output_sha256


def run_publication_replacement_worker(
    cli: Path,
    env: dict[str, str],
    project: Path,
    private_root: Path,
    nonce: str,
    *,
    timeout: int,
) -> None:
    request_path = private_root / "publication-replacement-worker-request.json"
    output_path = private_root / "publication-replacement-worker-output.json"
    write_private_json(
        request_path,
        {
            "schema_version": 1,
            "nonce_sha256": hashlib.sha256(nonce.encode("ascii")).hexdigest(),
            "executable_sha256": sha256(cli),
            "project": str(project.resolve()),
            "operation": "query",
            "parameters": {
                "query_count": 1,
                "bulk_count": 0,
                "documents_per_bulk": 0,
                "input_bytes": 64,
                "hold_ms": 0,
            },
        },
    )
    run(
        [
            str(cli),
            "internal-embedding-qualification-worker",
            "--request",
            str(request_path),
            "--output",
            str(output_path),
        ],
        env=env,
        cwd=project,
        timeout=timeout,
    )
    require(
        output_path.is_file() and not output_path.is_symlink(),
        "publication replacement worker omitted its output",
    )
    try:
        output = json.loads(output_path.read_text(encoding="utf-8"))
    except json.JSONDecodeError as exc:
        raise ProofFailure(
            f"publication replacement worker output is not valid JSON: {exc}"
        ) from exc
    require(
        isinstance(output, dict)
        and output.get("schema_version") == 1
        and output.get("executable_sha256") == sha256(cli)
        and output.get("error") is None,
        "publication replacement worker failed",
    )
    result = output.get("result")
    operations = result.get("operations") if isinstance(result, dict) else None
    require(
        isinstance(result, dict)
        and result.get("schema_version") == 1
        and result.get("scenario") == "query"
        and isinstance(operations, list)
        and len(operations) == 1
        and operations[0].get("status") == "ok"
        and operations[0].get("error_code") is None,
        "publication replacement worker did not complete its query",
    )


def produce_product_publication_fault_evidence(
    cli: Path,
    env: dict[str, str],
    private_root: Path,
    artifact_root: Path,
    nonce: str,
    *,
    source: dict,
    package: dict,
    contracts: dict,
    timeout: int,
) -> tuple[Path, Path]:
    project = private_root / "publication-product-repository"
    project.mkdir(mode=0o700)
    anchors = [
        f"qualification_anchor_{index:02d}"
        for index in range(FAULT_RECOVERY_CONSISTENCY_CASES)
    ]
    source_file = project / "lib.rs"
    baseline_source = (
        "\n".join(
            f'pub fn {anchor}() -> &\'static str {{ "{anchor}" }}' for anchor in anchors
        )
        + "\n"
    )
    source_file.write_text(baseline_source, encoding="utf-8")
    lexical_file = project / "README.md"
    baseline_lexical = "# Publication qualification baseline\n"
    lexical_file.write_text(baseline_lexical, encoding="utf-8")
    baseline_file_times = {
        path: (metadata.st_atime_ns, metadata.st_mtime_ns)
        for path in (source_file, lexical_file)
        for metadata in (path.stat(),)
    }
    run_id = "publication-qualification"
    index_command = [
        str(cli),
        "index",
        "--project",
        str(project),
        "--refresh",
        "full",
        "--format",
        "json",
    ]
    retrieval_index_command = [
        str(cli),
        "retrieval",
        "index",
        "--project",
        str(project),
        "--profile",
        "agent",
        "--run-id",
        run_id,
        "--refresh",
        "none",
        "--format",
        "json",
    ]
    status_command = [
        str(cli),
        "retrieval",
        "status",
        "--project",
        str(project),
        "--profile",
        "agent",
        "--run-id",
        run_id,
        "--format",
        "json",
    ]
    json_command(index_command, env=env, cwd=project, timeout=timeout)
    json_command(retrieval_index_command, env=env, cwd=project, timeout=timeout)
    baseline_status_result, baseline_status = json_command(
        status_command, env=env, cwd=project, timeout=timeout
    )
    previous_publication = publication_identity_from_status(baseline_status)
    baseline_ranks = []
    for anchor in anchors:
        rank, _ = run_quality_search(
            cli, env, project, run_id, anchor, anchor, timeout=timeout
        )
        baseline_ranks.append(rank)

    snapshot_before = send_server_qualification_control(
        private_root, nonce, sequence=1, action="snapshot", timeout=timeout
    )
    correlation_id = secrets.token_hex(16)
    nonce_sha256 = hashlib.sha256(nonce.encode("ascii")).hexdigest()
    pause_path = private_root / f"publication-pause-{nonce_sha256}.json"
    resume_path = private_root / f"publication-resume-{correlation_id}.json"
    hook_event_path = private_root / f"publication-events-{correlation_id}.jsonl"
    write_private_json(
        pause_path,
        {
            "schema_version": 1,
            "nonce_sha256": nonce_sha256,
            "correlation_id": correlation_id,
            "action": "pause_before_manifest_commit",
        },
    )
    source_file.write_text(
        baseline_source + "// publication qualification candidate source change\n",
        encoding="utf-8",
    )
    lexical_file.write_text("# Publication qualification candidate\n", encoding="utf-8")
    candidate = subprocess.Popen(
        retrieval_index_command,
        cwd=project,
        env=env,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
    )
    candidate_stdout = ""
    candidate_stderr = ""
    try:
        wait_for_jsonl_event(
            hook_event_path,
            lambda event: event.get("action") == "pause_before_manifest_commit"
            and event.get("status") == "waiting_for_resume",
            timeout=timeout,
            process=candidate,
        )
        send_server_qualification_control(
            private_root, nonce, sequence=2, action="crash_server", timeout=timeout
        )
        run_publication_replacement_worker(
            cli,
            env,
            project,
            private_root,
            nonce,
            timeout=timeout,
        )
        snapshot_after = send_server_qualification_control(
            private_root, nonce, sequence=3, action="snapshot", timeout=timeout
        )
        write_private_json(
            resume_path,
            {
                "schema_version": 1,
                "nonce_sha256": nonce_sha256,
                "correlation_id": correlation_id,
                "action": "resume_manifest_commit",
            },
        )
        candidate_stdout, candidate_stderr = candidate.communicate(timeout=timeout)
    except BaseException:
        if candidate.poll() is None:
            candidate.kill()
            candidate_stdout, candidate_stderr = candidate.communicate()
        raise
    finally:
        source_file.write_text(baseline_source, encoding="utf-8")
        lexical_file.write_text(baseline_lexical, encoding="utf-8")
        for path, times in baseline_file_times.items():
            os.utime(path, ns=times)
    require(
        candidate.returncode is not None and candidate.returncode != 0,
        "publication candidate did not fail after losing its server lease",
    )
    hook_events = read_jsonl(hook_event_path)
    require(len(hook_events) == 4, "publication hook did not emit its exact four events")
    final_status_result, final_status = json_command(
        status_command, env=env, cwd=project, timeout=timeout
    )
    final_publication = publication_identity_from_status(final_status)
    post_ranks = []
    post_search_sha256 = None
    for anchor in anchors:
        rank, output_sha256 = run_quality_search(
            cli, env, project, run_id, anchor, anchor, timeout=timeout
        )
        post_ranks.append(rank)
        if post_search_sha256 is None:
            post_search_sha256 = output_sha256
    require(post_search_sha256 is not None, "qualification search emitted no output digest")

    publication_payload = {
        "schema_version": 1,
        "evidence_contract": PUBLICATION_FAULT_EVIDENCE_CONTRACT,
        "source": source,
        "package": package,
        "contracts": contracts,
        "correlation_id": correlation_id,
        "previous_publication_identity_sha256": previous_publication,
        "server_observations": [
            server_observation_from_control_event(snapshot_before, "before_crash"),
            server_observation_from_control_event(snapshot_after, "after_replacement"),
        ],
        "candidate_observation": {
            "command": "retrieval_index",
            "exit_code": candidate.returncode,
            "stdout_sha256": hashlib.sha256(candidate_stdout.encode("utf-8")).hexdigest(),
            "stderr_sha256": hashlib.sha256(candidate_stderr.encode("utf-8")).hexdigest(),
        },
        "publication_hook_events": hook_events,
        "ordinary_product_observations": [
            {
                "sequence": 0,
                "command": "retrieval_status",
                "exit_code": 0,
                "retrieval_mode": final_status["retrieval_mode"],
                "publication_identity_sha256": final_publication,
                "output_sha256": hashlib.sha256(
                    final_status_result["stdout"].encode("utf-8")
                ).hexdigest(),
            },
            {
                "sequence": 1,
                "command": "search",
                "exit_code": 0,
                "retrieval_mode": final_status["retrieval_mode"],
                "publication_identity_sha256": final_publication,
                "output_sha256": post_search_sha256,
            },
        ],
    }
    publication_path = artifact_root / "publication-fault-external.raw.json"
    write_private_json(publication_path, publication_payload)
    consistency_payload = {
        "schema_version": 1,
        "evidence_contract": FAULT_RECOVERY_CONSISTENCY_CONTRACT,
        "source": source,
        "package": package,
        "contracts": contracts,
        "run_id_sha256": hashlib.sha256(correlation_id.encode("ascii")).hexdigest(),
        "observations": [
            {
                "case_id_sha256": hashlib.sha256(anchor.encode("utf-8")).hexdigest(),
                "before_server_fault_rank": baseline_ranks[index],
                "after_server_replacement_rank": post_ranks[index],
            }
            for index, anchor in enumerate(anchors)
        ],
    }
    consistency_path = artifact_root / "fault-recovery-consistency.raw.json"
    write_private_json(consistency_path, consistency_payload)
    for path in (pause_path, resume_path):
        try:
            path.unlink()
        except FileNotFoundError:
            pass
    return publication_path, consistency_path


def load_external_raw_evidence(path: Path, label: str) -> tuple[dict, str]:
    require(path.is_file() and not path.is_symlink(), f"{label} is missing or unsafe: {path}")
    metadata = path.stat()
    require(stat.S_ISREG(metadata.st_mode), f"{label} is not a regular file")
    require(metadata.st_size <= 8 * 1024 * 1024, f"{label} exceeds the 8 MiB evidence limit")
    payload_bytes = path.read_bytes()
    try:
        payload = json.loads(payload_bytes)
    except json.JSONDecodeError as exc:
        raise ProofFailure(f"{label} is not valid JSON: {exc}") from exc
    require(isinstance(payload, dict), f"{label} must be an object")
    return payload, hashlib.sha256(payload_bytes).hexdigest()


def verify_publication_fault_raw_evidence(
    path: Path,
    *,
    source: dict,
    package: dict,
    contracts: dict,
) -> dict:
    payload, artifact_sha256 = load_external_raw_evidence(
        path, "publication fault raw evidence"
    )
    require_exact_keys(
        payload,
        {
            "schema_version",
            "evidence_contract",
            "source",
            "package",
            "contracts",
            "correlation_id",
            "previous_publication_identity_sha256",
            "server_observations",
            "candidate_observation",
            "publication_hook_events",
            "ordinary_product_observations",
        },
        "publication fault raw evidence",
    )
    require(payload["schema_version"] == 1, "publication fault evidence schema is unsupported")
    require(
        payload["evidence_contract"] == PUBLICATION_FAULT_EVIDENCE_CONTRACT,
        "publication fault evidence contract is unsupported",
    )
    require(payload["source"] == source, "publication fault evidence source identity is stale")
    require(payload["package"] == package, "publication fault evidence package identity is stale")
    require(payload["contracts"] == contracts, "publication fault evidence contracts are stale")
    correlation_id = payload["correlation_id"]
    require(
        isinstance(correlation_id, str)
        and re.fullmatch(r"[0-9a-f]{32}", correlation_id) is not None,
        "publication fault correlation id is invalid",
    )
    previous_publication = require_sha256(
        payload["previous_publication_identity_sha256"],
        "publication fault previous publication identity",
    )

    server_observations = payload["server_observations"]
    require(
        isinstance(server_observations, list) and len(server_observations) == 2,
        "publication fault evidence requires before-crash and after-replacement server observations",
    )
    expected_server_phases = ("before_crash", "after_replacement")
    for index, (observation, phase) in enumerate(
        zip(server_observations, expected_server_phases)
    ):
        require(isinstance(observation, dict), f"server observation {index} is malformed")
        require_exact_keys(
            observation,
            {"phase", "server_instance_id", "process_start_id", "load_generation"},
            f"server observation {index}",
        )
        require(observation["phase"] == phase, f"server observation {index} has the wrong phase")
        require_opaque_identifier(
            observation["server_instance_id"],
            f"server observation {index}.server_instance_id",
        )
        require_opaque_identifier(
            observation["process_start_id"],
            f"server observation {index}.process_start_id",
        )
        require_positive_int(
            observation["load_generation"],
            f"server observation {index}.load_generation",
        )
    require(
        (
            server_observations[0]["server_instance_id"],
            server_observations[0]["process_start_id"],
        )
        != (
            server_observations[1]["server_instance_id"],
            server_observations[1]["process_start_id"],
        ),
        "publication fault evidence did not observe a replacement server",
    )

    candidate = payload["candidate_observation"]
    require(isinstance(candidate, dict), "publication candidate observation is malformed")
    require_exact_keys(
        candidate,
        {"command", "exit_code", "stdout_sha256", "stderr_sha256"},
        "publication candidate observation",
    )
    require(candidate["command"] == "retrieval_index", "publication candidate used the wrong command")
    require(
        isinstance(candidate["exit_code"], int)
        and not isinstance(candidate["exit_code"], bool)
        and candidate["exit_code"] != 0,
        "publication candidate unexpectedly committed successfully",
    )
    require_sha256(candidate["stdout_sha256"], "publication candidate stdout sha256")
    require_sha256(candidate["stderr_sha256"], "publication candidate stderr sha256")

    events = payload["publication_hook_events"]
    expected_events = (
        ("pause_before_manifest_commit", "waiting_for_resume"),
        ("resume_manifest_commit", "observed"),
        ("lease_revalidation", "failed"),
        ("manifest_commit", "returned_error"),
    )
    require(
        isinstance(events, list) and len(events) == len(expected_events),
        "publication hook evidence must contain the exact four raw fence events",
    )
    last_elapsed = -1
    for index, (event, expected) in enumerate(zip(events, expected_events)):
        require(isinstance(event, dict), f"publication hook event {index} is malformed")
        require_exact_keys(
            event,
            {"schema_version", "sequence", "correlation_id", "action", "status", "clock"},
            f"publication hook event {index}",
        )
        require(event["schema_version"] == 1, f"publication hook event {index} schema is unsupported")
        require(event["sequence"] == index, "publication hook event sequence is not exact")
        require(event["correlation_id"] == correlation_id, "publication hook correlation changed")
        require(
            (event["action"], event["status"]) == expected,
            f"publication hook event {index} does not match the fence contract",
        )
        clock = event["clock"]
        require(isinstance(clock, dict), f"publication hook event {index} omitted its clock")
        require_exact_keys(clock, {"domain", "api", "elapsed_ns"}, f"publication hook event {index} clock")
        require(
            clock["domain"] == "process_monotonic"
            and clock["api"] == "std::time::Instant",
            "publication hook used an unsupported clock",
        )
        elapsed = require_nonnegative_int(
            clock["elapsed_ns"], f"publication hook event {index} elapsed_ns"
        )
        require(elapsed >= last_elapsed, "publication hook elapsed time moved backwards")
        last_elapsed = elapsed

    ordinary = payload["ordinary_product_observations"]
    require(
        isinstance(ordinary, list) and len(ordinary) == 2,
        "publication fault evidence requires status and query product observations",
    )
    for index, (observation, command) in enumerate(
        zip(ordinary, ("retrieval_status", "search"))
    ):
        require(isinstance(observation, dict), f"ordinary product observation {index} is malformed")
        require_exact_keys(
            observation,
            {
                "sequence",
                "command",
                "exit_code",
                "retrieval_mode",
                "publication_identity_sha256",
                "output_sha256",
            },
            f"ordinary product observation {index}",
        )
        require(observation["sequence"] == index, "ordinary product observation order changed")
        require(observation["command"] == command, "ordinary product observation used the wrong command")
        require(observation["exit_code"] == 0, f"ordinary product {command} failed")
        require(observation["retrieval_mode"] == "full", f"ordinary product {command} was not full")
        require(
            require_sha256(
                observation["publication_identity_sha256"],
                f"ordinary product {command} publication identity",
            )
            == previous_publication,
            f"ordinary product {command} did not use the previous publication",
        )
        require_sha256(observation["output_sha256"], f"ordinary product {command} output sha256")

    return {
        "artifact": {
            "name": "publication-fault-external.raw.json",
            "sha256": artifact_sha256,
        },
        "assertions": {
            "lost_publication_lease_blocks_commit": True,
            "previous_publication_remains_usable": True,
        },
    }


def verify_fault_recovery_consistency_raw_evidence(
    path: Path,
    *,
    source: dict,
    package: dict,
    contracts: dict,
) -> dict:
    payload, artifact_sha256 = load_external_raw_evidence(
        path, "fault recovery consistency raw evidence"
    )
    require_exact_keys(
        payload,
        {
            "schema_version",
            "evidence_contract",
            "source",
            "package",
            "contracts",
            "run_id_sha256",
            "observations",
        },
        "fault recovery consistency raw evidence",
    )
    require(
        payload["schema_version"] == 1,
        "fault recovery consistency evidence schema is unsupported",
    )
    require(
        payload["evidence_contract"] == FAULT_RECOVERY_CONSISTENCY_CONTRACT,
        "fault recovery consistency evidence contract is unsupported",
    )
    require(
        payload["source"] == source,
        "fault recovery consistency source identity is stale",
    )
    require(
        payload["package"] == package,
        "fault recovery consistency package identity is stale",
    )
    require(
        payload["contracts"] == contracts,
        "fault recovery consistency contracts are stale",
    )
    require_sha256(payload["run_id_sha256"], "fault recovery consistency run id")
    observations = payload["observations"]
    require(
        isinstance(observations, list)
        and len(observations) == FAULT_RECOVERY_CONSISTENCY_CASES,
        "fault recovery consistency evidence has the wrong case count",
    )
    case_ids: set[str] = set()
    for index, observation in enumerate(observations):
        require(
            isinstance(observation, dict),
            f"fault recovery consistency observation {index} is malformed",
        )
        require_exact_keys(
            observation,
            {
                "case_id_sha256",
                "before_server_fault_rank",
                "after_server_replacement_rank",
            },
            f"fault recovery consistency observation {index}",
        )
        case_id = require_sha256(
            observation["case_id_sha256"],
            f"fault recovery consistency observation {index} case id",
        )
        require(
            case_id not in case_ids,
            "fault recovery consistency evidence contains duplicate cases",
        )
        case_ids.add(case_id)
        for field in ("before_server_fault_rank", "after_server_replacement_rank"):
            rank = observation[field]
            require(
                rank is None
                or (
                    isinstance(rank, int)
                    and not isinstance(rank, bool)
                    and 1 <= rank <= 10
                ),
                f"fault recovery consistency observation {index} {field} is not a rank in the fixed top 10",
            )
        require(
            observation["before_server_fault_rank"]
            == observation["after_server_replacement_rank"],
            "fault recovery changed a search rank from the retained publication",
        )
    return {
        "artifact": {
            "name": "fault-recovery-consistency.raw.json",
            "sha256": artifact_sha256,
        },
        "case_count": len(observations),
        "ranks_stable": True,
    }


def verify_retrieval_quality_raw_evidence(
    path: Path,
    *,
    source: dict,
    holdout_task_root: Path = HOLDOUT_TASK_ROOT,
) -> dict:
    payload, artifact_sha256 = load_external_raw_evidence(
        path, "publishable packet quality raw evidence"
    )
    release_evidence = payload.get("release_evidence")
    require(
        isinstance(release_evidence, dict),
        "publishable packet evidence omitted release_evidence",
    )
    for field in ("assertions", "accepted", "decision"):
        require(
            field not in payload and field not in release_evidence,
            f"publishable packet evidence contains self-declared {field}",
        )
    require(
        release_evidence.get("commit") == source["commit"],
        "publishable packet evidence source commit is stale",
    )
    require(
        release_evidence.get("source_tree") == source["tree"],
        "publishable packet evidence source tree is stale",
    )
    require(
        release_evidence.get("evaluation_contract")
        == RETRIEVAL_QUALITY_EVIDENCE_CONTRACT,
        "publishable packet evaluation contract is unsupported",
    )
    holdout_tasks, holdout_manifest_set_sha256 = load_holdout_task_contracts(
        holdout_task_root
    )
    evidence_identity = release_evidence.get("evidence_identity")
    require(
        isinstance(evidence_identity, dict)
        and evidence_identity.get("corpus_id") == RELEASE_QUALITY_CORPUS_ID,
        "publishable packet evidence is not bound to the release holdout corpus",
    )
    repeats = require_positive_int(
        release_evidence.get("repeats"),
        "publishable packet repeat count",
    )
    require(
        repeats == MIN_RETRIEVAL_QUALITY_REPEATS,
        f"publishable packet evidence requires exactly {MIN_RETRIEVAL_QUALITY_REPEATS} repeats",
    )
    require(
        release_evidence.get("publishable") is True,
        "packet quality artifact is not publishable",
    )
    require(
        release_evidence.get("quality_gate_status") == "pass",
        "packet quality artifact did not pass its quality gate",
    )
    blockers = release_evidence.get("publishable_blockers")
    require(
        isinstance(blockers, list) and not blockers,
        "packet quality artifact contains publishable blockers",
    )
    require(
        payload.get("repeats") == repeats,
        "packet quality top-level repeat count changed",
    )
    modes = payload.get("modes")
    require(
        isinstance(modes, list) and modes == list(RELEASE_QUALITY_MODES),
        "packet quality artifact must contain only the release cold-cli mode",
    )
    expected_modes = set(RELEASE_QUALITY_MODES.values())
    expected_cells = {
        (repo, task_id, mode, repeat)
        for repo, task_id in holdout_tasks
        for mode in expected_modes
        for repeat in range(1, MIN_RETRIEVAL_QUALITY_REPEATS + 1)
    }

    rows = release_evidence.get("rows")
    require(
        isinstance(rows, list) and rows,
        "packet quality artifact has no quality rows",
    )
    observed_cells: set[tuple[str, str, str, int]] = set()
    passing_rows = 0
    for index, row in enumerate(rows):
        require(isinstance(row, dict), f"packet quality row {index} is malformed")
        quality = row.get("quality")
        sufficiency = row.get("sufficiency")
        latency = row.get("packet_latency")
        require(
            row.get("status") == "pass"
            and isinstance(quality, dict)
            and quality.get("pass") is True,
            f"packet quality row {index} did not pass",
        )
        require(
            isinstance(sufficiency, dict)
            and sufficiency.get("status") == "sufficient"
            and sufficiency.get("sufficient_quality_mismatch") is not True,
            f"packet quality row {index} is not sufficient",
        )
        for field in (
            "follow_up_commands_count",
            "open_next_count",
            "gaps_count",
            "coverage_unresolved_blocking_count",
        ):
            value = sufficiency.get(field, 0)
            require(
                isinstance(value, (int, float))
                and not isinstance(value, bool)
                and value == 0,
                f"packet quality row {index} has unresolved {field}",
            )
        require(
            isinstance(latency, dict)
            and latency.get("sla_missed") is False
            and isinstance(latency.get("retrieval_shadow"), dict)
            and latency["retrieval_shadow"].get("retrieval_mode") == "full",
            f"packet quality row {index} lacks full-retrieval latency proof",
        )

        provenance = row.get("repo_provenance")
        require(
            isinstance(provenance, dict)
            and provenance.get("manifest_overridden_by_builtin") is False
            and provenance.get("git_dirty") is False,
            f"packet quality row {index} has untrusted repository provenance",
        )
        configured = provenance.get("configured")
        manifest_repo = provenance.get("manifest")
        require(
            isinstance(configured, dict) and isinstance(manifest_repo, dict),
            f"packet quality row {index} omitted repository identities",
        )
        configured_ref = configured.get("ref")
        require(
            isinstance(configured_ref, str)
            and re.fullmatch(r"[0-9a-f]{40}", configured_ref) is not None
            and manifest_repo.get("ref") == configured_ref
            and provenance.get("git_head") == configured_ref,
            f"packet quality row {index} is not pinned to one immutable repository commit",
        )
        trusted_repo_url = re.compile(
            r"^https://github\.com/[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+(?:\.git)?$"
        )
        urls = (
            configured.get("url"),
            manifest_repo.get("url"),
            provenance.get("git_origin"),
        )
        require(
            all(
                isinstance(url, str) and trusted_repo_url.fullmatch(url) is not None
                for url in urls
            ),
            f"packet quality row {index} has an untrusted repository URL",
        )
        normalized_urls = {
            re.sub(r"\.git$", "", url, flags=re.IGNORECASE).lower()
            for url in urls
        }
        require(
            len(normalized_urls) == 1,
            f"packet quality row {index} repository URLs disagree",
        )

        cache = row.get("codestory_cache_provenance")
        require(
            isinstance(cache, dict)
            and cache.get("doctor_status") == "pass"
            and bool(cache.get("storage_path"))
            and bool(cache.get("cache_policy"))
            and cache.get("cache_policy") != "unprepared-cache-blocked"
            and cache.get("retrieval_mode") == "full"
            and bool(cache.get("semantic_generation"))
            and bool(cache.get("manifest_embedding_backend"))
            and bool(cache.get("embedding_engine_instance_id"))
            and cache.get("embedding_policy") in {"accelerated", "cpu_explicit"}
            and cache.get("semantic_backend") is not None
            and cache.get("local_only") is True
            and cache.get("indexed") is True
            and cache.get("freshness_status") == "fresh"
            and cache.get("semantic_ready") is True
            and cache.get("indexing_in_timed_run") is not None,
            f"packet quality row {index} has incomplete CodeStory cache provenance",
        )

        repeat = row.get("repeat")
        require(
            isinstance(repeat, int)
            and not isinstance(repeat, bool)
            and 1 <= repeat <= repeats,
            f"packet quality row {index} has an invalid repeat",
        )
        repo = require_nonempty_string(row.get("repo"), f"packet quality row {index} repo")
        task_id = require_nonempty_string(
            row.get("task_id"), f"packet quality row {index} task id"
        )
        mode = require_nonempty_string(
            row.get("mode"), f"packet quality row {index} mode"
        )
        require(
            mode in expected_modes,
            f"packet quality row {index} mode is not declared at top level",
        )
        task_contract = holdout_tasks.get((repo, task_id))
        require(
            task_contract is not None,
            f"packet quality row {index} is not one of the checked-in holdout tasks",
        )
        snapshot = row.get("task_manifest_snapshot")
        require(
            isinstance(snapshot, dict),
            f"packet quality row {index} omitted its task manifest snapshot",
        )
        snapshot_without_path = {
            key: value for key, value in snapshot.items() if key != "manifest_path"
        }
        require(
            snapshot_without_path == task_contract["snapshot"],
            f"packet quality row {index} task snapshot differs from the checked-in manifest",
        )
        manifest_path = snapshot.get("manifest_path")
        require(
            isinstance(manifest_path, str)
            and Path(manifest_path).name == task_contract["path"].name,
            f"packet quality row {index} names a different task manifest",
        )
        expected_repo = task_contract["repo"]
        require(
            configured.get("url") == expected_repo["url"]
            and configured.get("ref") == expected_repo["ref"]
            and configured.get("languages") == expected_repo.get("languages", [])
            and manifest_repo.get("url") == expected_repo["url"]
            and manifest_repo.get("ref") == expected_repo["ref"]
            and manifest_repo.get("workspace_root") == expected_repo.get("workspace_root"),
            f"packet quality row {index} repository identity differs from its checked-in task",
        )
        cell = (repo, task_id, mode, repeat)
        require(
            cell not in observed_cells,
            f"packet quality rows duplicate repeat {repeat} for {repo}/{task_id}/{mode}",
        )
        observed_cells.add(cell)
        passing_rows += 1

    require(
        observed_cells == expected_cells,
        "packet quality rows do not exactly cover the checked-in repo/task/mode/repeat matrix",
    )
    pass_rate = passing_rows / len(rows)
    return {
        "artifact": {
            "name": "packet-runtime-summary.json",
            "sha256": artifact_sha256,
        },
        "evaluation_contract": RETRIEVAL_QUALITY_EVIDENCE_CONTRACT,
        "source_commit": source["commit"],
        "source_tree": source["tree"],
        "corpus_id": RELEASE_QUALITY_CORPUS_ID,
        "holdout_manifest_set_sha256": holdout_manifest_set_sha256,
        "repeats": repeats,
        "row_count": len(rows),
        "passing_row_count": passing_rows,
        "publishable_packet_pass_rate": pass_rate,
    }
