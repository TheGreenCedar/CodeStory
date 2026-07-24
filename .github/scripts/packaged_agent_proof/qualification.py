"""Qualification for packaged CodeStory proof."""

from .foundation import *
from .contracts import (
    ProofFailure,
    qualification_measurement_sample_value,
    require,
    require_exact_keys,
    require_nonempty_string,
    require_nonnegative_int,
    require_opaque_identifier,
    require_positive_int,
    require_sha256,
    selected_qualification_matrix_cell,
)
from .archive import (
    normalized_backend,
)

def verify_retained_qualification(
    evidence: dict,
    *,
    manifest: dict,
    archive_sha256: str,
    shared_identity: dict,
    measurement_contract: dict,
    required_tier: str,
    required_matrix_cell_id: str,
    expected_policy: str,
    expected_backend: str,
    expected_accelerator_claim: str,
    installed_plugin: dict | None = None,
    managed_runtime: dict | None = None,
) -> dict:
    require(
        evidence.get("schema_version") == QUALIFICATION_SCHEMA_VERSION,
        "retained qualification schema is unsupported",
    )
    require(evidence.get("status") == "pass", "retained qualification is not a passing result")
    tier = evidence.get("tier")
    require(
        tier in {"hosted_package", "protected_hardware", "installed_runtime"},
        "retained qualification tier is invalid",
    )
    require(
        tier == required_tier,
        f"retained {tier} evidence cannot support exact requested tier {required_tier}",
    )
    matrix_cell = selected_qualification_matrix_cell(
        measurement_contract["measurement_protocol"],
        cell_id=required_matrix_cell_id,
        target=manifest["asset_target"],
        proof_tier=required_tier,
        expected_policy=expected_policy,
        expected_backend=expected_backend,
    )
    require(
        matrix_cell["accelerator_claim"] == expected_accelerator_claim,
        "requested accelerator claim does not match the selected qualification matrix cell",
    )
    retained_plugin = evidence.get("installed_plugin")
    retained_runtime = evidence.get("managed_runtime")
    if tier == "installed_runtime":
        require(isinstance(retained_plugin, dict), "installed evidence omitted plugin provenance")
        require(isinstance(retained_runtime, dict), "installed evidence omitted managed runtime provenance")
        installation_source = retained_plugin.get("installation_source")
        require(
            retained_plugin.get("schema_version") == 2
            and installation_source
            in {"codex_marketplace_install", "candidate_archive"}
            and retained_plugin.get("plugin_id") == "codestory"
            and retained_plugin.get("plugin_version") == manifest["release_version"],
            "installed evidence has invalid plugin provenance",
        )
        if installation_source == "codex_marketplace_install":
            require(
                retained_plugin.get("marketplace_repository")
                == "TheGreenCedar/AgentPluginMarketplace"
                and retained_plugin.get("codex_cli_version")
                == PINNED_CODEX_CLI_VERSION
                and retained_runtime.get("build_source") == "github_release"
                and retained_runtime.get("repo_ref")
                == f"v{manifest['release_version']}",
                "installed evidence has invalid marketplace/release provenance",
            )
            require(
                isinstance(retained_plugin.get("marketplace_commit"), str)
                and re.fullmatch(
                    r"[0-9a-f]{40}",
                    retained_plugin["marketplace_commit"],
                )
                is not None,
                "installed evidence marketplace commit is invalid",
            )
        else:
            producer = retained_plugin.get("producer")
            require(
                retained_plugin.get("candidate_archive_sha256")
                == archive_sha256
                and retained_plugin.get("candidate_asset_target")
                == manifest["asset_target"]
                and retained_plugin.get("plugin_source_tree")
                == manifest["source"]["tree"]
                and retained_runtime.get("build_source")
                == "candidate_archive"
                and retained_runtime.get("repo_ref")
                == manifest["source"]["commit"],
                "installed evidence has invalid staged-candidate provenance",
            )
            require(
                isinstance(producer, dict)
                and producer.get("repository") == "TheGreenCedar/CodeStory"
                and producer.get("workflow_path")
                in CANDIDATE_PRODUCER_WORKFLOW_PATHS
                and isinstance(producer.get("run_id"), str)
                and re.fullmatch(r"[1-9][0-9]*", producer["run_id"]) is not None
                and isinstance(producer.get("run_attempt"), str)
                and re.fullmatch(r"[1-9][0-9]*", producer["run_attempt"])
                is not None,
                "installed evidence has unauthenticated candidate producer identity",
            )
        require_sha256(
            retained_plugin.get("plugin_package_sha256"),
            "installed evidence plugin_package_sha256",
        )
        require(
            retained_plugin.get("plugin_source_commit") == manifest["source"]["commit"],
            "installed evidence does not bind the marketplace plugin to the packaged source commit",
        )
        require(
            retained_runtime.get("cli_source") == "managed"
            and retained_runtime.get("plugin_version") == manifest["release_version"]
            and retained_runtime.get("managed_binary_sha256") == manifest["binary"]["sha256"]
            and retained_runtime.get("archive_sha256") == archive_sha256,
            "installed evidence does not bind the exact managed runtime",
        )
        if installed_plugin is not None:
            require(retained_plugin == installed_plugin, "retained installed plugin provenance is stale")
        if managed_runtime is not None:
            require(retained_runtime == managed_runtime, "retained managed runtime provenance is stale")
    else:
        require(
            retained_plugin is None and retained_runtime is None,
            "lower-tier evidence must not claim installed plugin provenance",
        )

    source = evidence.get("source")
    require(isinstance(source, dict), "retained qualification omitted source identity")
    require(source == manifest["source"], "retained qualification source identity does not match package")

    package = evidence.get("package")
    require(isinstance(package, dict), "retained qualification omitted package identity")
    require(
        package.get("archive_sha256") == archive_sha256,
        "retained qualification names a different archive",
    )
    require(
        package.get("executable_sha256") == manifest["binary"]["sha256"],
        "retained qualification names a different executable",
    )
    require(
        package.get("asset_target") == manifest["asset_target"],
        "retained qualification names a different package target",
    )
    require(
        package.get("release_version") == manifest["release_version"],
        "retained qualification names a different release version",
    )
    require(
        package.get("model_sha256") == manifest["model"]["sha256"],
        "retained qualification names a different model",
    )
    require(
        package.get("matrix_cell_id") == required_matrix_cell_id
        and package.get("policy") == expected_policy
        and normalized_backend(package.get("backend"))
        == normalized_backend(expected_backend)
        and package.get("accelerator_claim") == expected_accelerator_claim,
        "retained qualification package does not match the requested matrix cell, policy, backend, or accelerator claim",
    )
    for field in (
        "protocol_sha256",
        "constant_set_sha256",
        "measurement_protocol_sha256",
    ):
        require(
            package.get(field) == manifest["server_proof"][field],
            f"retained qualification {field} does not match package",
        )

    host = evidence.get("host")
    require(isinstance(host, dict), "retained qualification omitted host identity")
    require_sha256(host.get("fingerprint"), "retained qualification host fingerprint")
    require_nonempty_string(host.get("platform"), "retained qualification host platform")
    require(
        host.get("target") == manifest["asset_target"],
        "retained qualification host names a different package target",
    )
    require_nonempty_string(host.get("backend"), "retained qualification host backend")
    require(
        host.get("matrix_cell_id") == required_matrix_cell_id
        and host.get("accelerator_claim") == expected_accelerator_claim
        and host.get("host_class") == matrix_cell["host_class"],
        "retained qualification host does not match the requested matrix cell",
    )
    require(
        normalized_backend(package.get("backend")) == normalized_backend(host["backend"]),
        "retained qualification package and host backend identities disagree",
    )
    require(
        package.get("policy") == host.get("policy")
        and host.get("policy") in {"accelerated", "cpu_explicit"},
        "retained qualification package and host policy identities disagree",
    )
    require(
        host.get("policy") == expected_policy
        and normalized_backend(host.get("backend"))
        == normalized_backend(expected_backend),
        "retained qualification host used the wrong requested policy or backend",
    )
    for field in ("cache_state", "residency_state"):
        require_nonempty_string(host.get(field), f"retained qualification host {field}")
        require(
            package.get(field) == host[field],
            f"retained qualification package and host {field} disagree",
        )
        require(
            host[field] == matrix_cell[field],
            f"retained qualification host {field} differs from the selected matrix cell",
        )
    require(
        host.get("unplanned_suspend") is False,
        "retained qualification host recorded an unplanned suspend",
    )

    same_account = evidence.get("same_account")
    require(isinstance(same_account, dict), "retained qualification omitted same-account evidence")
    require_nonempty_string(same_account.get("account_id"), "same_account.account_id")
    require(
        same_account.get("relation") == "same_os_account",
        "retained qualification does not prove same-OS-account scope",
    )
    hosts = same_account.get("plugin_hosts")
    require(isinstance(hosts, list) and len(hosts) == 2, "qualification requires exactly two plugin hosts")
    host_ids: set[tuple[object, object]] = set()
    repository_ids: set[str] = set()
    for index, host in enumerate(hosts):
        require(isinstance(host, dict), f"plugin host {index} is malformed")
        require_positive_int(host.get("pid"), f"plugin_hosts[{index}].pid")
        start_id = require_nonempty_string(
            host.get("process_start_id"),
            f"plugin_hosts[{index}].process_start_id",
        )
        repository_id = require_nonempty_string(
            host.get("repository_id"),
            f"plugin_hosts[{index}].repository_id",
        )
        require(
            not Path(repository_id).is_absolute(),
            "retained plugin-host evidence must use an opaque repository identity, not a path",
        )
        host_ids.add((host["pid"], start_id))
        repository_ids.add(repository_id)
    require(len(host_ids) == 2, "plugin hosts are not independently started processes")
    require(len(repository_ids) == 2, "plugin hosts did not use different repositories")
    require(
        same_account.get("cross_login_or_terminal_sessions_proven") is False,
        "base same-account evidence must not infer cross-session sharing",
    )

    retained_shared = evidence.get("shared_identity")
    require(isinstance(retained_shared, dict), "retained qualification omitted shared server identity")
    require(
        isinstance(shared_identity, dict),
        "live two-host proof omitted shared server identity",
    )
    for field in (
        "endpoint_namespace_id",
        "lifetime_authority_id",
        "listener_id",
        "server_instance_id",
        "server_process_start_id",
        "engine_owner_id",
        "native_worker_id",
        "load_generation",
        "model_load_count",
    ):
        require(field in retained_shared, f"retained shared identity omitted {field}")
        require(
            retained_shared[field] == shared_identity[field],
            f"retained shared identity {field} does not match the live two-host proof",
        )
    require(retained_shared["model_load_count"] == 1, "retained cold race did not prove one model load")

    timing = evidence.get("timing")
    require(isinstance(timing, dict), "retained qualification omitted timing identity")
    require(timing.get("clock_domain") == "awake_monotonic", "qualification used the wrong clock domain")
    require(timing.get("cross_process_timestamp_subtraction") is False, "qualification subtracted cross-process timestamps")
    require(timing.get("unplanned_suspend") is False, "qualification performance block included suspend")
    require(timing.get("constants_frozen_before_run") is True, "qualification selected constants from its own results")
    require(
        timing.get("constant_set_sha256") == manifest["server_proof"]["constant_set_sha256"],
        "qualification timing used a different constant set",
    )

    scenarios = evidence.get("scenarios")
    require(isinstance(scenarios, dict), "retained qualification omitted scenario evidence")
    scenario_contracts = measurement_contract["measurement_protocol"]["scenario_contracts"]
    require(set(scenarios) == REQUIRED_SERVER_SCENARIOS, "retained qualification scenario set is incomplete")
    for scenario_id in sorted(REQUIRED_SERVER_SCENARIOS):
        scenario = scenarios.get(scenario_id)
        require(isinstance(scenario, dict), f"scenario {scenario_id} is malformed")
        require(scenario.get("status") == "pass", f"scenario {scenario_id} did not pass")
        assertions = scenario.get("assertions")
        require(isinstance(assertions, dict), f"scenario {scenario_id} omitted assertions")
        required_assertions = set(scenario_contracts[scenario_id]["required"])
        require(
            set(assertions) == required_assertions,
            f"scenario {scenario_id} assertions do not match the preregistered contract",
        )
        failed = sorted(name for name, passed in assertions.items() if passed is not True)
        require(not failed, f"scenario {scenario_id} has failed assertions: " + ", ".join(failed))
        artifacts = scenario.get("artifacts")
        require(isinstance(artifacts, list) and artifacts, f"scenario {scenario_id} has no retained artifacts")
        artifact_names: set[str] = set()
        for artifact in artifacts:
            require(isinstance(artifact, dict), f"scenario {scenario_id} artifact is malformed")
            name = require_nonempty_string(artifact.get("name"), f"scenario {scenario_id} artifact name")
            require(
                Path(name).name == name and Path(name).suffix == ".json",
                f"scenario {scenario_id} artifact name is not a safe JSON basename",
            )
            require_sha256(artifact.get("sha256"), f"scenario {scenario_id} artifact sha256")
            artifact_names.add(name)
        if scenario_id in {"server_crash", "worker_stall"}:
            require(
                "publication-fault-external.raw.json" in artifact_names,
                f"{scenario_id} scenario omitted separately hashed publication-fence evidence",
            )

    nonclaims = evidence.get("lower_tier_nonclaims")
    require(isinstance(nonclaims, dict), "retained qualification omitted lower-tier nonclaims")
    require(set(nonclaims) == LOWER_TIER_NONCLAIMS, "retained qualification nonclaim set is incomplete")
    for claim, record in nonclaims.items():
        require(isinstance(record, dict), f"nonclaim {claim} is malformed")
        require(record.get("claimed") is False, f"lower-tier evidence incorrectly claims {claim}")
        require_nonempty_string(record.get("reason"), f"nonclaim {claim} reason")

    metrics = evidence.get("metrics")
    require(isinstance(metrics, dict), "retained qualification omitted metric results")
    required_metrics = set(measurement_contract["measurement_protocol"]["required_metrics"])
    require(set(metrics) == required_metrics, "retained qualification metric set is incomplete")
    thresholds = measurement_contract["constant_set"]["qualification_thresholds"]
    metric_contracts = measurement_contract["measurement_protocol"]["metric_contracts"]
    for metric, result in metrics.items():
        require(isinstance(result, dict), f"metric {metric} is malformed")
        require(result.get("status") == "pass", f"metric {metric} did not pass its frozen threshold")
        require(
            result.get("unit") == metric_contracts[metric]["unit"],
            f"metric {metric} used the wrong unit",
        )
        require(
            isinstance(result.get("value"), (int, float))
            and not isinstance(result.get("value"), bool),
            f"metric {metric} value is not numeric",
        )
        require(
            result.get("threshold") == thresholds[metric]
            and isinstance(result.get("threshold"), (int, float))
            and not isinstance(result.get("threshold"), bool),
            f"metric {metric} threshold does not match the frozen constant set",
        )
        comparison = metric_contracts[metric]["comparison"]
        require(result.get("comparison") == comparison, f"metric {metric} used the wrong comparison")
        if metric == "retrieval_quality":
            raw_evidence = result.get("raw_evidence")
            require(isinstance(raw_evidence, dict), "retrieval quality metric omitted raw evidence")
            require_exact_keys(
                raw_evidence,
                {
                    "artifact",
                    "evaluation_contract",
                    "source_commit",
                    "source_tree",
                    "corpus_id",
                    "holdout_manifest_set_sha256",
                    "repeats",
                    "row_count",
                    "passing_row_count",
                    "publishable_packet_pass_rate",
                },
                "retrieval quality retained raw evidence",
            )
            artifact = raw_evidence["artifact"]
            require(isinstance(artifact, dict), "retrieval quality raw artifact is malformed")
            require_exact_keys(artifact, {"name", "sha256"}, "retrieval quality raw artifact")
            require(
                artifact["name"] == "packet-runtime-summary.json",
                "retrieval quality raw artifact name is invalid",
            )
            require_sha256(artifact["sha256"], "retrieval quality raw artifact sha256")
            require(
                raw_evidence["evaluation_contract"] == RETRIEVAL_QUALITY_EVIDENCE_CONTRACT,
                "retrieval quality retained evaluation contract changed",
            )
            require(
                raw_evidence["source_commit"] == evidence["source"]["commit"]
                and raw_evidence["source_tree"] == evidence["source"]["tree"],
                "retrieval quality retained source identity is stale",
            )
            require(
                require_positive_int(raw_evidence["repeats"], "retrieval quality repeats")
                == MIN_RETRIEVAL_QUALITY_REPEATS,
                "retrieval quality retained the wrong repeat count",
            )
            require(
                raw_evidence["corpus_id"] == RELEASE_QUALITY_CORPUS_ID,
                "retrieval quality retained the wrong holdout corpus",
            )
            require_sha256(
                raw_evidence["holdout_manifest_set_sha256"],
                "retrieval quality holdout manifest set sha256",
            )
            row_count = require_positive_int(
                raw_evidence["row_count"], "retrieval quality row count"
            )
            require(
                require_positive_int(
                    raw_evidence["passing_row_count"],
                    "retrieval quality passing row count",
                )
                == row_count,
                "retrieval quality retained a failing row",
            )
            require(
                isinstance(raw_evidence["publishable_packet_pass_rate"], (int, float))
                and not isinstance(raw_evidence["publishable_packet_pass_rate"], bool),
                "retrieval quality pass rate is not numeric",
            )
            require(
                raw_evidence["publishable_packet_pass_rate"] == result["value"],
                "retrieval quality metric does not match its raw evidence",
            )
        else:
            raw_evidence = result.get("raw_evidence")
            require(
                isinstance(raw_evidence, dict),
                f"metric {metric} omitted its raw measurement artifact",
            )
            require_exact_keys(
                raw_evidence,
                {"name", "sha256"},
                f"metric {metric} raw measurement artifact",
            )
            expected_artifact_name = (
                "total-codestory-process-memory.raw.json"
                if metric == "total_codestory_process_memory"
                else "measurements.raw.json"
            )
            require(
                raw_evidence["name"] == expected_artifact_name,
                f"metric {metric} used the wrong raw measurement artifact",
            )
            require_sha256(raw_evidence["sha256"], f"metric {metric} raw artifact sha256")
        passed = {
            "equal": result["value"] == result["threshold"],
            "greater_than_or_equal": result["value"] >= result["threshold"],
            "less_than_or_equal": result["value"] <= result["threshold"],
        }[comparison]
        require(passed, f"metric {metric} value failed its frozen comparison")

    return evidence


def derive_scenario_assertions(
    scenario_id: str,
    *,
    observations_by_kind: dict[str, list[dict]],
    process_observations: list[dict],
    invocations: list[dict],
    control_actions: list[str],
    same_account: dict,
    materialization: dict,
) -> dict[str, bool]:
    def transition(kind: str, expected_keys: set[str] | None = None) -> dict:
        matches = observations_by_kind.get(kind, [])
        require(
            len(matches) == 1,
            f"qualification scenario {scenario_id} omitted or duplicated transition {kind}",
        )
        values = matches[0]["values"]
        require(isinstance(values, dict), f"qualification transition {kind} values are malformed")
        if expected_keys is not None:
            require_exact_keys(values, expected_keys, f"qualification transition {kind} values")
        return values

    def scheduler(kind: str) -> dict:
        values = transition(
            kind,
            {
                "query_capacity",
                "query_depth",
                "bulk_capacity",
                "bulk_depth",
                "active_request_count",
                "lease_count",
                "active_request_class",
            },
        )
        for field in (
            "query_capacity",
            "query_depth",
            "bulk_capacity",
            "bulk_depth",
            "active_request_count",
            "lease_count",
        ):
            require_nonnegative_int(values[field], f"qualification transition {kind}.{field}")
        require(
            values["active_request_class"] in {None, "query", "bulk"},
            f"qualification transition {kind} has an invalid active request class",
        )
        return values

    def replay_attempts(
        values: dict,
        *,
        old_server_instance_id: str,
        new_server_instance_id: str,
    ) -> list[dict]:
        attempts = values["wire_attempts"]
        require(
            values["wire_attempt_count"] == 2
            and isinstance(attempts, list)
            and len(attempts) == 2,
            "replay evidence must contain exactly the original RPC and one replay",
        )
        for index, attempt in enumerate(attempts, start=1):
            require(isinstance(attempt, dict), "replay attempt is malformed")
            require_exact_keys(
                attempt,
                {
                    "ordinal",
                    "request_id",
                    "server_instance_id",
                    "submitted_ns",
                    "completed_ns",
                    "outcome",
                },
                f"replay attempt {index}",
            )
            require(attempt["ordinal"] == index, "replay attempt ordinal is not exact")
            require(
                isinstance(attempt["request_id"], str) and bool(attempt["request_id"]),
                "replay attempt request ID is missing",
            )
            submitted = require_nonnegative_int(
                attempt["submitted_ns"], f"replay attempt {index} submitted_ns"
            )
            completed = require_nonnegative_int(
                attempt["completed_ns"], f"replay attempt {index} completed_ns"
            )
            require(completed >= submitted, "replay attempt clock moved backwards")
        require(
            attempts[0]["request_id"] != attempts[1]["request_id"]
            and attempts[0]["server_instance_id"] == old_server_instance_id
            and attempts[0]["outcome"] == "server_loss"
            and attempts[1]["server_instance_id"] == new_server_instance_id
            and attempts[1]["outcome"] == "completed",
            "replay attempts do not bind the old loss and exact replacement completion",
        )
        return attempts

    def retry_state(value: object, field: str) -> dict:
        require(isinstance(value, dict), f"{field} is malformed")
        expected = {
            "code",
            "message_head",
            "retry_class",
            "retry_after_ms",
            "retry_condition",
        }
        if "capacity" in value:
            expected.add("capacity")
        require_exact_keys(value, expected, field)
        require_nonempty_string(value["code"], f"{field}.code")
        require_nonempty_string(value["message_head"], f"{field}.message_head")
        require_nonempty_string(value["retry_class"], f"{field}.retry_class")
        require_nonnegative_int(value["retry_after_ms"], f"{field}.retry_after_ms")
        require_nonempty_string(value["retry_condition"], f"{field}.retry_condition")
        require(
            value["retry_class"]
            in {
                "after_capacity_change",
                "after_delay",
                "after_owner_idle",
                "after_server_change",
                "none",
                "same_rpc_once",
                "terminal",
            },
            f"{field}.retry_class is outside the protocol contract",
        )
        return value

    snapshots = [
        observation["snapshot"]
        for observation in process_observations
        if observation.get("snapshot") is not None
    ]
    snapshot_instances = {
        snapshot["process"]["server_instance_id"] for snapshot in snapshots
    }
    snapshot_authorities = {
        (
            snapshot["authority"]["lifetime_authority_id"],
            snapshot["authority"]["listener_id"],
        )
        for snapshot in snapshots
    }
    snapshot_engines = {
        (
            snapshot["engine"]["engine_owner_id"],
            snapshot["engine"]["native_worker_id"],
            snapshot["engine"]["load_generation"],
            snapshot["engine"]["model_load_count"],
        )
        for snapshot in snapshots
        if snapshot.get("engine") is not None
    }

    assertions: dict[str, bool]
    if scenario_id == "client_death":
        active = scheduler("dead_client_work_observed")
        continued = transition("other_client_continued", {"project_identity_sha256"})
        terminated = transition("client_terminated", {"termination"})
        reclaimed = scheduler("dead_client_work_reclaimed")
        post = transition("post_reclaim_other_client_query", {"server_instance_id"})
        assertions = {
            "dead_client_queue_and_leases_reclaimed": (
                active["query_depth"] > 0
                and active["bulk_depth"] > 0
                and active["active_request_count"] > 0
                and active["lease_count"] > 0
                and reclaimed["query_depth"] == 0
                and reclaimed["bulk_depth"] == 0
                and reclaimed["active_request_count"] == 0
                and reclaimed["lease_count"] == 0
                and terminated["termination"] == "terminated"
            ),
            "other_client_continues": (
                HEX_SHA256.fullmatch(str(continued["project_identity_sha256"])) is not None
                and post["server_instance_id"] in snapshot_instances
            ),
            "no_server_replacement": len(snapshot_instances) == 1,
        }
    elif scenario_id == "cold_race":
        election_witnesses = {
            phase: [
                observation
                for observation in process_observations
                if observation.get("phase") == phase
            ]
            for phase in ("cold_race_first", "cold_race_second")
        }
        require(
            all(len(witnesses) == 1 for witnesses in election_witnesses.values()),
            "cold race must retain exactly one post-reset snapshot from each process",
        )
        election_snapshots = [
            election_witnesses[phase][0]["snapshot"]
            for phase in ("cold_race_first", "cold_race_second")
        ]
        require(
            all(
                isinstance(snapshot, dict) and snapshot.get("engine") is not None
                for snapshot in election_snapshots
            ),
            "cold race post-reset snapshots must retain engine identity",
        )
        election_instances = {
            snapshot["process"]["server_instance_id"]
            for snapshot in election_snapshots
        }
        election_authorities = {
            (
                snapshot["authority"]["lifetime_authority_id"],
                snapshot["authority"]["listener_id"],
            )
            for snapshot in election_snapshots
        }
        election_engines = {
            (
                snapshot["engine"]["engine_owner_id"],
                snapshot["engine"]["native_worker_id"],
                snapshot["engine"]["load_generation"],
                snapshot["engine"]["model_load_count"],
            )
            for snapshot in election_snapshots
        }
        independent = transition(
            "two_independent_processes",
            {
                "first_pid",
                "second_pid",
                "first_project_identity_sha256",
                "second_project_identity_sha256",
                "first_transport_peer_verified",
                "second_transport_peer_verified",
            },
        )
        converged = transition(
            "single_server_convergence",
            {"server_instance_id", "lifetime_authority_id"},
        )
        hosts = same_account.get("plugin_hosts") if isinstance(same_account, dict) else None
        assertions = {
            "two_independent_plugin_hosts": (
                require_positive_int(independent["first_pid"], "cold race first pid")
                != require_positive_int(independent["second_pid"], "cold race second pid")
                and independent["first_transport_peer_verified"] is True
                and independent["second_transport_peer_verified"] is True
            ),
            "same_os_account": (
                same_account.get("relation") == "same_os_account"
                and isinstance(hosts, list)
                and len(hosts) == 2
            ),
            "different_repositories": (
                independent["first_project_identity_sha256"]
                != independent["second_project_identity_sha256"]
                and all(
                    HEX_SHA256.fullmatch(str(independent[field])) is not None
                    for field in (
                        "first_project_identity_sha256",
                        "second_project_identity_sha256",
                    )
                )
            ),
            "one_lifetime_authority": (
                len(election_authorities) == 1
                and converged["lifetime_authority_id"]
                == next(iter(election_authorities))[0]
            ),
            "one_listener": len({identity[1] for identity in election_authorities}) == 1,
            "one_server": (
                len(election_instances) == 1
                and converged["server_instance_id"] == next(iter(election_instances))
            ),
            "one_engine_owner": len({identity[0] for identity in election_engines}) == 1,
            "one_native_worker": len({identity[1] for identity in election_engines}) == 1,
            "one_load_generation": len({identity[2] for identity in election_engines}) == 1,
            "one_model_load": (
                len(election_engines) == 1 and next(iter(election_engines))[3] == 1
            ),
        }
    elif scenario_id == "frozen_owner":
        bounded = transition(
            "bounded_owner_unresponsive",
            {
                "started_ns",
                "finished_ns",
                "error_code",
                "timeout_ms",
                "clock_domain",
                "clock_boot_id",
                "retry",
            },
        )
        stable = transition(
            "owner_identity_stable",
            {
                "server_instance_id",
                "lifetime_authority_id",
                "listener_id",
                "pid",
                "process_start_id",
                "post_release_query_succeeded",
            },
        )
        started = require_nonnegative_int(bounded["started_ns"], "frozen owner started_ns")
        finished = require_nonnegative_int(bounded["finished_ns"], "frozen owner finished_ns")
        timeout_ms = require_positive_int(bounded["timeout_ms"], "frozen owner timeout_ms")
        stable_pid = require_positive_int(stable["pid"], "frozen owner stable pid")
        retry = retry_state(bounded["retry"], "frozen owner retry")
        stable_identity = (
            len(snapshot_instances) == 1
            and stable["server_instance_id"] == next(iter(snapshot_instances))
            and len(snapshot_authorities) == 1
            and (
                stable["lifetime_authority_id"],
                stable["listener_id"],
            )
            == next(iter(snapshot_authorities))
            and all(
                snapshot["process"]["pid"] == stable_pid
                and snapshot["process"]["process_start_id"]
                == stable["process_start_id"]
                for snapshot in snapshots
            )
            and stable["post_release_query_succeeded"] is True
        )
        assertions = {
            "owner_unresponsive_is_bounded": (
                finished >= started
                and finished - started <= timeout_ms * 1_000_000
                and bounded["clock_domain"] == "awake_monotonic"
                and bool(bounded["clock_boot_id"])
                and bounded["error_code"] == "embedding_server_owner_unresponsive"
                and retry["code"] == bounded["error_code"]
                and retry["retry_class"] == "after_server_change"
                and bool(retry["retry_condition"])
            ),
            "authority_retained": stable_identity,
            "no_unlink": stable_identity,
            "no_pid_kill": stable_identity,
            "no_takeover": stable_identity,
            "no_second_engine": len(snapshot_engines) == 1,
        }
    elif scenario_id == "incompatible_owner":
        active = transition(
            "active_owner_rejected",
            {"compatibility_evidence", "error_code", "retry"},
        )
        idle = transition(
            "idle_owner_draining",
            {"compatibility_evidence", "error_code", "retry"},
        )
        replacement = transition(
            "compatible_replacement",
            {"old_server_instance_id", "new_server_instance_id"},
        )
        replaced = (
            replacement["old_server_instance_id"] != replacement["new_server_instance_id"]
            and {
                replacement["old_server_instance_id"],
                replacement["new_server_instance_id"],
            }
            <= snapshot_instances
        )
        active_retry = retry_state(active["retry"], "incompatible active retry")
        idle_retry = retry_state(idle["retry"], "incompatible idle retry")
        assertions = {
            "idle_owner_drains": (
                idle["compatibility_evidence"] == "injected_contract_mismatch"
                and idle["error_code"] == "embedding_server_draining"
                and idle_retry["code"] == idle["error_code"]
                and idle_retry["retry_class"] == "after_owner_idle"
                and idle_retry["retry_after_ms"] == 0
                and idle_retry["retry_condition"]
                == "the incompatible server exits while fully idle"
                and replaced
            ),
            "active_owner_returns_typed_retry": (
                active["compatibility_evidence"] == "injected_contract_mismatch"
                and active["error_code"] == "embedding_server_incompatible_active_owner"
                and active_retry["code"] == active["error_code"]
                and active_retry["retry_class"] == "after_owner_idle"
                and active_retry["retry_after_ms"] == 0
                and active_retry["retry_condition"]
                == "the incompatible server exits while fully idle"
            ),
            "one_authority": len(snapshot_authorities) <= 2 and replaced,
            "one_engine_maximum": len(snapshot_instances) == 2 and replaced,
        }
    elif scenario_id == "mixed_queue":
        saturated = scheduler("queues_saturated")
        selected = scheduler("query_selected_before_bulk_backlog")
        capacity = transition("typed_capacity_retry_observed", {"query_65th", "bulk_65th"})
        class_orders = transition(
            "per_class_fifo_observed",
            {
                "query_expected_queue_insertion_request_ids",
                "query_native_completed_request_ids",
                "query_native_completion_sequences",
                "bulk_expected_queue_insertion_request_ids",
                "bulk_native_completed_request_ids",
                "bulk_native_completion_sequences",
            },
        )
        project_orders = transition(
            "global_fifo_across_projects",
            {
                "query_expected_queue_insertion_project_identities",
                "query_native_completed_project_identities",
                "bulk_expected_queue_insertion_project_identities",
                "bulk_native_completed_project_identities",
            },
        )
        preference = transition(
            "query_preference_observed",
            {
                "first_query_request_id",
                "first_query_native_completion_sequence",
                "first_bulk_request_id",
                "first_bulk_native_completion_sequence",
            },
        )
        resumed = transition(
            "bulk_resumed",
            {
                "last_query_request_id",
                "last_query_native_completion_sequence",
                "last_bulk_request_id",
                "last_bulk_native_completion_sequence",
            },
        )
        typed_capacity = True
        for queue_class in ("query", "bulk"):
            record = capacity[f"{queue_class}_65th"]
            pressure = record.get("error", {}).get("capacity") if isinstance(record, dict) else None
            typed_capacity = typed_capacity and (
                isinstance(pressure, dict)
                and pressure.get("queue_class") == queue_class
                and pressure.get("capacity") == 64
                and pressure.get("depth") == 64
                and bool(pressure.get("retry_condition"))
            )
        fifo = all(
            class_orders[f"{queue_class}_expected_queue_insertion_request_ids"]
            == class_orders[f"{queue_class}_native_completed_request_ids"]
            and isinstance(
                class_orders[f"{queue_class}_expected_queue_insertion_request_ids"], list
            )
            and bool(class_orders[f"{queue_class}_expected_queue_insertion_request_ids"])
            and isinstance(class_orders[f"{queue_class}_native_completion_sequences"], list)
            and bool(class_orders[f"{queue_class}_native_completion_sequences"])
            and all(
                isinstance(sequence, int)
                and not isinstance(sequence, bool)
                and sequence > 0
                for sequence in class_orders[f"{queue_class}_native_completion_sequences"]
            )
            and class_orders[f"{queue_class}_native_completion_sequences"]
            == sorted(class_orders[f"{queue_class}_native_completion_sequences"])
            and len(set(class_orders[f"{queue_class}_native_completion_sequences"]))
            == len(class_orders[f"{queue_class}_native_completion_sequences"])
            for queue_class in ("query", "bulk")
        )
        global_fifo = all(
            project_orders[f"{queue_class}_expected_queue_insertion_project_identities"]
            == project_orders[f"{queue_class}_native_completed_project_identities"]
            and len(
                set(project_orders[f"{queue_class}_expected_queue_insertion_project_identities"])
            )
            == 2
            for queue_class in ("query", "bulk")
        )
        assertions = {
            "query_and_bulk_capacities_are_64": (
                saturated["query_capacity"] == saturated["query_depth"] == 64
                and saturated["bulk_capacity"] == saturated["bulk_depth"] == 64
            ),
            "fifo_within_each_class": fifo,
            "query_preferred_between_bulk_batches": (
                selected["active_request_class"] == "query"
                and selected["bulk_depth"] > 0
                and preference["first_query_native_completion_sequence"]
                < preference["first_bulk_native_completion_sequence"]
            ),
            "bulk_resumes_when_query_queue_permits": (
                resumed["last_bulk_native_completion_sequence"]
                > resumed["last_query_native_completion_sequence"]
            ),
            "no_project_or_scope_round_robin": global_fifo,
            "typed_retry_names_useful_condition": typed_capacity,
            "no_project_or_request_text_leakage": all(
                all(
                    HEX_SHA256.fullmatch(str(value)) is not None
                    for value in values
                )
                for key, values in project_orders.items()
                if key.endswith("_project_identities")
            ),
        }
    elif scenario_id == "server_crash":
        active = scheduler("inflight_request_observed")
        replacement = transition(
            "server_replaced",
            {"old_server_instance_id", "new_server_instance_id"},
        )
        replay = transition(
            "query_replayed",
            {
                "logical_operation_count",
                "wire_attempt_count",
                "wire_attempts",
            },
        )
        attempts = replay_attempts(
            replay,
            old_server_instance_id=replacement["old_server_instance_id"],
            new_server_instance_id=replacement["new_server_instance_id"],
        )
        assertions = {
            "one_replacement_server": (
                active["active_request_class"] == "query"
                and replacement["old_server_instance_id"]
                != replacement["new_server_instance_id"]
                and [attempt["server_instance_id"] for attempt in attempts]
                == [
                    replacement["old_server_instance_id"],
                    replacement["new_server_instance_id"],
                ]
            ),
            "pure_embedding_rpc_replayed_at_most_once": (
                replay["logical_operation_count"] == 1
                and replay["wire_attempt_count"] <= 2
                and sum(attempt["outcome"] == "completed" for attempt in attempts) == 1
            ),
        }
    elif scenario_id == "true_idle_respawn":
        active = scheduler("anti_idle_work_observed")
        preserved = transition(
            "owner_preserved_across_idle_boundary",
            {
                "held_started_ns",
                "held_observed_ns",
                "contract_idle_timeout_ms",
                "server_instance_id",
            },
        )
        reclaimed = scheduler("anti_idle_work_reclaimed")
        waited = transition(
            "true_idle_wait",
            {
                "server_idle_epoch_ns",
                "server_idle_elapsed_before_client_wait_ns",
                "client_wait_required_ns",
                "client_wait_elapsed_ns",
                "contract_idle_timeout_ms",
                "clock_boot_id",
            },
        )
        idle_surfaces = transition(
            "idle_surfaces_exercised",
            {
                "diagnostic_count",
                "idle_connection_close_count",
                "last_diagnostic_client_elapsed_ns",
                "last_idle_connection_close_client_elapsed_ns",
            },
        )
        absent = transition("owner_absent_after_true_idle", {"old_server_instance_id"})
        respawned = transition(
            "server_respawned",
            {
                "new_server_instance_id",
                "load_generation",
                "model_load_count",
                "materialized_model_sha256",
                "materialized_reused",
            },
        )
        absent_transition_observation = observations_by_kind[
            "owner_absent_after_true_idle"
        ][0]
        respawn_transition_observation = observations_by_kind["server_respawned"][0]
        timeout_ms = require_positive_int(
            waited["contract_idle_timeout_ms"],
            "true idle contract timeout",
        )
        diagnostic_count = require_positive_int(
            idle_surfaces["diagnostic_count"],
            "true idle diagnostic count",
        )
        idle_connection_close_count = require_positive_int(
            idle_surfaces["idle_connection_close_count"],
            "true idle connection close count",
        )
        last_diagnostic_client_elapsed_ns = require_nonnegative_int(
            idle_surfaces["last_diagnostic_client_elapsed_ns"],
            "true idle last diagnostic ns",
        )
        last_idle_connection_close_client_elapsed_ns = require_nonnegative_int(
            idle_surfaces["last_idle_connection_close_client_elapsed_ns"],
            "true idle last connection close ns",
        )
        absent_observations = [
            observation
            for observation in process_observations
            if observation.get("phase") == "true_idle_after_wait"
        ]
        require(
            len(absent_observations) == 1
            and absent_observations[0].get("snapshot") is None,
            "true idle must retain exactly one absent-owner witness",
        )
        respawn_observations = [
            observation
            for observation in process_observations
            if observation.get("phase") == "true_idle_respawned"
        ]
        require(
            len(respawn_observations) == 1
            and isinstance(respawn_observations[0].get("snapshot"), dict)
            and respawn_observations[0]["snapshot"].get("engine") is not None,
            "true idle must retain exactly one replacement-engine witness",
        )
        absent_observed_ns = require_nonnegative_int(
            absent_observations[0].get("observed_ns"),
            "true idle absent-owner witness time",
        )
        respawn_observed_ns = require_nonnegative_int(
            respawn_observations[0].get("observed_ns"),
            "true idle replacement witness time",
        )
        respawn_snapshot = respawn_observations[0]["snapshot"]
        respawn_engine = respawn_snapshot["engine"]
        absent_transition_ns = require_nonnegative_int(
            absent_transition_observation.get("observed_ns"),
            "true idle absence transition time",
        )
        respawn_transition_ns = require_nonnegative_int(
            respawn_transition_observation.get("observed_ns"),
            "true idle respawn transition time",
        )
        post_absence_invocations = [
            invocation
            for invocation in invocations
            if isinstance(invocation.get("started_ns"), int)
            and not isinstance(invocation.get("started_ns"), bool)
            and absent_transition_ns <= invocation["started_ns"]
            <= respawn_transition_ns
        ]
        absent_observed = (
            len(absent_observations) == 1
            and absent_observations[0].get("snapshot") is None
        )
        respawn_load_generation = require_positive_int(
            respawned["load_generation"],
            "true idle respawn load generation",
        )
        respawn_model_load_count = require_positive_int(
            respawned["model_load_count"],
            "true idle respawn model load count",
        )
        respawn_materialized_sha256 = require_sha256(
            respawned["materialized_model_sha256"],
            "true idle respawn materialized model",
        )
        require_nonnegative_int(
            waited["server_idle_epoch_ns"], "true idle server epoch"
        )
        server_idle_elapsed_before_client_wait_ns = require_nonnegative_int(
            waited["server_idle_elapsed_before_client_wait_ns"],
            "true idle server elapsed before local wait",
        )
        client_wait_required_ns = require_nonnegative_int(
            waited["client_wait_required_ns"], "true idle client wait required"
        )
        client_wait_elapsed_ns = require_nonnegative_int(
            waited["client_wait_elapsed_ns"], "true idle client wait elapsed"
        )
        assertions = {
            "queued_active_and_leased_work_prevent_exit": (
                active["query_depth"] > 0
                and active["bulk_depth"] > 0
                and active["active_request_count"] > 0
                and active["lease_count"] > 0
                and preserved["server_instance_id"] == absent["old_server_instance_id"]
                and preserved["held_observed_ns"] - preserved["held_started_ns"]
                >= preserved["contract_idle_timeout_ms"] * 1_000_000
            ),
            "idle_connections_and_diagnostics_do_not_extend_idle": (
                diagnostic_count >= 2
                and idle_connection_close_count >= 2
                and last_diagnostic_client_elapsed_ns
                >= timeout_ms * 500_000
                and last_idle_connection_close_client_elapsed_ns
                >= timeout_ms * 500_000
                and bool(waited["clock_boot_id"])
                and absent_observed
            ),
            "exit_after_60000_awake_ms": (
                timeout_ms == 60_000
                and server_idle_elapsed_before_client_wait_ns
                + client_wait_required_ns
                >= timeout_ms * 1_000_000
                and client_wait_elapsed_ns >= client_wait_required_ns
                and reclaimed["query_depth"] == 0
                and reclaimed["bulk_depth"] == 0
                and reclaimed["active_request_count"] == 0
                and reclaimed["lease_count"] == 0
                and absent_observed
            ),
            "next_product_operation_respawns_without_consent": (
                absent["old_server_instance_id"] != respawned["new_server_instance_id"]
                and absent_observed_ns <= absent_transition_ns
                and len(post_absence_invocations) == 1
                and post_absence_invocations[0].get("operation") == "query"
                and post_absence_invocations[0].get("exit_code") == 0
                and post_absence_invocations[0].get("termination") == "exited"
                and isinstance(post_absence_invocations[0].get("finished_ns"), int)
                and not isinstance(
                    post_absence_invocations[0].get("finished_ns"), bool
                )
                and post_absence_invocations[0]["started_ns"]
                <= post_absence_invocations[0]["finished_ns"]
                <= respawn_observed_ns <= respawn_transition_ns
                and respawn_snapshot["process"]["server_instance_id"]
                == respawned["new_server_instance_id"]
            ),
            "verified_materialization_reused": (
                isinstance(materialization, dict)
                and respawn_materialized_sha256
                == require_sha256(
                    materialization.get("sha256"),
                    "retained materialized model",
                )
                and respawned["materialized_reused"] is True
                and respawn_load_generation
                == respawn_engine["load_generation"]
                and respawn_model_load_count
                == respawn_engine["model_load_count"] == 1
            ),
        }
    else:
        require(scenario_id == "worker_stall", f"unknown qualification scenario {scenario_id}")
        active = scheduler("stalled_request_observed")
        fail_stop = transition(
            "watchdog_fail_stop_observed",
            {
                "old_pid",
                "old_server_instance_id",
                "wire_attempt_count",
                "wire_attempts",
                "watchdog_marker_sha256",
                "watchdog_reason",
                "watchdog_observed_ns",
                "watchdog_last_progress_ns",
                "watchdog_progress_sequence",
                "hard_native_no_progress_ms",
                "watchdog_cadence_ms",
            },
        )
        replacement = transition(
            "post_stall_replacement",
            {"new_server_instance_id"},
        )
        survivor = transition(
            "unrelated_process_survived",
            {"pid", "process_start_id", "new_server_instance_id"},
        )
        old_pid = require_positive_int(fail_stop["old_pid"], "worker stall old pid")
        marker_sha256 = require_sha256(
            fail_stop["watchdog_marker_sha256"], "worker stall watchdog marker"
        )
        watchdog_observed_ns = require_nonnegative_int(
            fail_stop["watchdog_observed_ns"], "worker stall watchdog observed_ns"
        )
        watchdog_last_progress_ns = require_nonnegative_int(
            fail_stop["watchdog_last_progress_ns"],
            "worker stall watchdog last progress",
        )
        hard_no_progress_ms = require_positive_int(
            fail_stop["hard_native_no_progress_ms"],
            "worker stall hard no-progress bound",
        )
        watchdog_cadence_ms = require_positive_int(
            fail_stop["watchdog_cadence_ms"], "worker stall watchdog cadence"
        )
        require_nonnegative_int(
            fail_stop["watchdog_progress_sequence"],
            "worker stall watchdog progress sequence",
        )
        attempts = replay_attempts(
            fail_stop,
            old_server_instance_id=fail_stop["old_server_instance_id"],
            new_server_instance_id=replacement["new_server_instance_id"],
        )
        replacement_observed = (
            replacement["new_server_instance_id"] in snapshot_instances
            and all(snapshot["process"]["pid"] != old_pid for snapshot in snapshots[-1:])
        )
        assertions = {
            "independent_watchdog_fail_stops_server": (
                active["active_request_class"] == "bulk"
                and bool(marker_sha256)
                and fail_stop["watchdog_reason"] == "embedding_engine_stalled"
                and watchdog_observed_ns >= watchdog_last_progress_ns
                and watchdog_observed_ns - watchdog_last_progress_ns
                >= hard_no_progress_ms * 1_000_000
                and watchdog_cadence_ms < hard_no_progress_ms
                and attempts[0]["outcome"] == "server_loss"
                and replacement_observed
            ),
            "unrelated_process_survives": (
                replacement_observed
                and require_positive_int(survivor["pid"], "worker stall survivor pid")
                != old_pid
                and bool(survivor["process_start_id"])
                and survivor["new_server_instance_id"]
                == replacement["new_server_instance_id"]
                and any(
                    invocation.get("pid") == survivor["pid"]
                    and invocation.get("process_start_id")
                    == survivor["process_start_id"]
                    and invocation.get("operation") == "query"
                    and invocation.get("termination") == "exited"
                    for invocation in invocations
                )
            ),
            "pure_embedding_rpc_replayed_at_most_once": (
                fail_stop["wire_attempt_count"] <= 2
                and sum(attempt["outcome"] == "completed" for attempt in attempts) == 1
            ),
        }
    failed = sorted(name for name, value in assertions.items() if value is not True)
    require(
        not failed,
        f"qualification scenario {scenario_id} raw evidence failed assertions: {', '.join(failed)}",
    )
    return assertions


def qualification_artifact(
    artifact_root: Path,
    summary: object,
    *,
    scenario_id: str,
    contracts: dict,
    package: dict,
    same_account: dict,
    materialization: dict,
    nonce_sha256: str,
    forbidden_values: list[str],
) -> tuple[dict, dict]:
    require(
        isinstance(summary, dict),
        f"qualification scenario {scenario_id} summary is malformed",
    )
    require_exact_keys(
        summary,
        {
            "artifact",
            "process_count",
            "control_event_count",
            "process_observation_count",
            "observation_count",
            "event_count",
        },
        f"qualification scenario {scenario_id} summary",
    )
    name = require_nonempty_string(
        summary["artifact"],
        f"qualification scenario {scenario_id} artifact",
    )
    relative = Path(name)
    require(
        not relative.is_absolute()
        and len(relative.parts) == 1
        and relative.name == name
        and relative.suffix == ".json",
        f"qualification scenario {scenario_id} artifact must be a JSON basename",
    )
    path = artifact_root / relative
    require(path.is_file() and not path.is_symlink(), f"qualification artifact is missing or unsafe: {name}")
    require(
        path.resolve().parent == artifact_root.resolve(),
        f"qualification artifact escaped its private output directory: {name}",
    )
    payload_bytes = path.read_bytes()
    for forbidden in forbidden_values:
        require(
            forbidden.encode("utf-8") not in payload_bytes,
            f"qualification artifact {name} leaked private request material",
        )
    try:
        payload = json.loads(payload_bytes)
    except json.JSONDecodeError as exc:
        raise ProofFailure(f"qualification artifact {name} is not valid JSON: {exc}") from exc
    require(isinstance(payload, dict), f"qualification artifact {name} must be an object")
    require_exact_keys(
        payload,
        {
            "schema_version",
            "scenario",
            "contracts",
            "orchestration",
            "control_events",
            "process_observations",
            "observations",
            "events",
        },
        f"qualification artifact {name}",
    )
    require(payload["schema_version"] == 3, f"qualification artifact {name} schema is unsupported")
    require(payload["scenario"] == scenario_id, f"qualification artifact {name} names the wrong scenario")
    require(payload["contracts"] == contracts, f"qualification artifact {name} used different contracts")

    orchestration = payload["orchestration"]
    require(
        isinstance(orchestration, dict),
        f"qualification artifact {name} orchestration is malformed",
    )
    require_exact_keys(
        orchestration,
        {"started_ns", "finished_ns", "process_invocations"},
        f"qualification artifact {name} orchestration",
    )
    started_ns = require_nonnegative_int(
        orchestration["started_ns"],
        f"qualification artifact {name} orchestration.started_ns",
    )
    finished_ns = require_nonnegative_int(
        orchestration["finished_ns"],
        f"qualification artifact {name} orchestration.finished_ns",
    )
    require(
        finished_ns >= started_ns,
        f"qualification artifact {name} orchestration moved backwards",
    )
    invocations = orchestration["process_invocations"]
    require(
        isinstance(invocations, list),
        f"qualification artifact {name} process invocations are malformed",
    )
    invocation_ids: set[str] = set()
    for index, invocation in enumerate(invocations):
        require(
            isinstance(invocation, dict),
            f"qualification artifact {name} process invocation {index} is malformed",
        )
        require_exact_keys(
            invocation,
            {
                "invocation_id",
                "operation",
                "project_identity_sha256",
                "pid",
                "process_start_id",
                "started_ns",
                "finished_ns",
                "exit_code",
                "termination",
            },
            f"qualification artifact {name} process invocation {index}",
        )
        invocation_id = require_nonempty_string(
            invocation["invocation_id"],
            f"qualification artifact {name} process invocation {index}.invocation_id",
        )
        require(
            invocation_id not in invocation_ids,
            f"qualification artifact {name} duplicated invocation {invocation_id}",
        )
        invocation_ids.add(invocation_id)
        require_nonempty_string(
            invocation["operation"],
            f"qualification artifact {name} process invocation {index}.operation",
        )
        require_sha256(
            invocation["project_identity_sha256"],
            f"qualification artifact {name} process invocation {index}.project_identity_sha256",
        )
        require_positive_int(
            invocation["pid"],
            f"qualification artifact {name} process invocation {index}.pid",
        )
        require_nonempty_string(
            invocation["process_start_id"],
            f"qualification artifact {name} process invocation {index}.process_start_id",
        )
        invocation_started = require_nonnegative_int(
            invocation["started_ns"],
            f"qualification artifact {name} process invocation {index}.started_ns",
        )
        invocation_finished = require_nonnegative_int(
            invocation["finished_ns"],
            f"qualification artifact {name} process invocation {index}.finished_ns",
        )
        require(
            started_ns <= invocation_started <= invocation_finished <= finished_ns,
            f"qualification artifact {name} process invocation {index} escaped its block",
        )
        require(
            invocation["exit_code"] is None
            or (
                isinstance(invocation["exit_code"], int)
                and not isinstance(invocation["exit_code"], bool)
            ),
            f"qualification artifact {name} process invocation {index}.exit_code is invalid",
        )
        require(
            invocation["termination"] in {"exited", "terminated"},
            f"qualification artifact {name} process invocation {index}.termination is invalid",
        )

    def validate_clock(clock: object, field: str, *, observed: bool) -> None:
        require(isinstance(clock, dict), f"{field} is malformed")
        expected = {"domain", "api", "boot_id", "resolution_ns"}
        if observed:
            expected = {"domain", "api", "boot_id", "observed_ns"}
        require_exact_keys(clock, expected, field)
        require(clock["domain"] == "awake_monotonic", f"{field} used the wrong clock domain")
        require_nonempty_string(clock["api"], f"{field}.api")
        require_nonempty_string(clock["boot_id"], f"{field}.boot_id")
        numeric = "observed_ns" if observed else "resolution_ns"
        require_nonnegative_int(clock[numeric], f"{field}.{numeric}")

    def validate_snapshot(snapshot: object, field: str) -> None:
        require(isinstance(snapshot, dict), f"{field} is malformed")
        required = {
            "schema_version",
            "event_sequence",
            "lifecycle",
            "clock",
            "protocol",
            "authority",
            "process",
            "scheduler",
        }
        require(
            required <= set(snapshot)
            and set(snapshot) <= required | {"engine", "failure"},
            f"{field} fields differ from the raw snapshot contract",
        )
        require(snapshot["schema_version"] == 1, f"{field} schema is unsupported")
        require_nonnegative_int(snapshot["event_sequence"], f"{field}.event_sequence")
        require(snapshot["lifecycle"] in SERVER_LIFECYCLES, f"{field} lifecycle is invalid")
        validate_clock(snapshot["clock"], f"{field}.clock", observed=False)
        protocol = snapshot["protocol"]
        require(isinstance(protocol, dict), f"{field}.protocol is malformed")
        for contract_field, expected in contracts.items():
            require(
                protocol.get(contract_field) == expected,
                f"{field}.protocol.{contract_field} is stale",
            )
        authority = snapshot["authority"]
        process = snapshot["process"]
        scheduler = snapshot["scheduler"]
        require(
            isinstance(authority, dict)
            and isinstance(process, dict)
            and isinstance(scheduler, dict),
            f"{field} omitted server identity",
        )
        require_exact_keys(
            process,
            {
                "server_instance_id",
                "pid",
                "process_start_id",
                "executable_sha256",
                "executable_version",
            },
            f"{field}.process",
        )
        for identity_field in (
            "endpoint_namespace_id",
            "lifetime_authority_id",
            "listener_id",
        ):
            require_nonempty_string(authority.get(identity_field), f"{field}.authority.{identity_field}")
        require_nonempty_string(
            process.get("server_instance_id"),
            f"{field}.process.server_instance_id",
        )
        require_positive_int(process.get("pid"), f"{field}.process.pid")
        require_nonempty_string(process.get("process_start_id"), f"{field}.process.process_start_id")
        require(
            process.get("executable_sha256") == package["executable_sha256"]
            and process.get("executable_version") == package["release_version"],
            f"{field}.process does not match the exact packaged executable",
        )
        require(
            scheduler.get("query_capacity") == 64 and scheduler.get("bulk_capacity") == 64,
            f"{field} queue capacities differ from the bound contract",
        )
        engine = snapshot.get("engine")
        if engine is not None:
            require(isinstance(engine, dict), f"{field}.engine is malformed")
            for identity_field in ("engine_owner_id", "native_worker_id"):
                require_nonempty_string(engine.get(identity_field), f"{field}.engine.{identity_field}")
            require_positive_int(engine.get("load_generation"), f"{field}.engine.load_generation")
            require_positive_int(engine.get("model_load_count"), f"{field}.engine.model_load_count")

    control_events = payload["control_events"]
    require(
        isinstance(control_events, list),
        f"qualification artifact {name} control events are malformed",
    )
    previous_control_sequence = -1
    control_actions: list[str] = []
    for index, event in enumerate(control_events):
        require(
            isinstance(event, dict),
            f"qualification artifact {name} control event {index} is malformed",
        )
        required = {
            "schema_version",
            "sequence",
            "action",
            "status",
            "authenticated_nonce_sha256",
            "server_event_sequence",
            "clock",
        }
        require(
            required <= set(event)
            and set(event) <= required | {"snapshot", "details"},
            f"qualification artifact {name} control event {index} fields are invalid",
        )
        require(event["schema_version"] == 1, f"qualification artifact {name} control event schema is unsupported")
        sequence = require_nonnegative_int(
            event["sequence"],
            f"qualification artifact {name} control event {index}.sequence",
        )
        require(
            sequence > previous_control_sequence,
            f"qualification artifact {name} control event sequence is not increasing",
        )
        previous_control_sequence = sequence
        action = require_nonempty_string(
            event["action"],
            f"qualification artifact {name} control event {index}.action",
        )
        require(
            action
            in {
                "crash_server",
                "stall_native",
                "release_native",
                "hold_class",
                "release_class",
                "force_incompatible",
                "clear_incompatible",
                "snapshot",
                "freeze_owner",
                "release_owner",
            },
            f"qualification artifact {name} used unknown control {action}",
        )
        control_actions.append(action)
        require(
            event["status"] in {"completed", "accepted"},
            f"qualification artifact {name} control event {index} did not complete",
        )
        require(
            event["authenticated_nonce_sha256"] == nonce_sha256,
            f"qualification artifact {name} control event {index} was not authenticated",
        )
        require_nonnegative_int(
            event["server_event_sequence"],
            f"qualification artifact {name} control event {index}.server_event_sequence",
        )
        validate_clock(
            event["clock"],
            f"qualification artifact {name} control event {index}.clock",
            observed=True,
        )
        if "snapshot" in event:
            validate_snapshot(
                event["snapshot"],
                f"qualification artifact {name} control event {index}.snapshot",
            )
        if "details" in event:
            require(
                isinstance(event["details"], dict)
                and all(
                    isinstance(key, str) and isinstance(value, str)
                    for key, value in event["details"].items()
                ),
                f"qualification artifact {name} control event {index}.details is malformed",
            )

    process_observations = payload["process_observations"]
    require(
        isinstance(process_observations, list),
        f"qualification artifact {name} process observations are malformed",
    )
    observation_fields = {
        "phase",
        "observed_ns",
        "server_instance_id",
        "pid",
        "process_start_id",
        "executable_sha256",
        "executable_version",
        "endpoint_namespace_id",
        "lifetime_authority_id",
        "listener_id",
        "protocol_sha256",
        "constant_set_sha256",
        "measurement_protocol_sha256",
        "load_generation",
        "snapshot",
    }
    for index, observation in enumerate(process_observations):
        require(
            isinstance(observation, dict),
            f"qualification artifact {name} process observation {index} is malformed",
        )
        require_exact_keys(
            observation,
            observation_fields,
            f"qualification artifact {name} process observation {index}",
        )
        require_nonempty_string(
            observation["phase"],
            f"qualification artifact {name} process observation {index}.phase",
        )
        observed_ns = require_nonnegative_int(
            observation["observed_ns"],
            f"qualification artifact {name} process observation {index}.observed_ns",
        )
        require(
            started_ns <= observed_ns <= finished_ns,
            f"qualification artifact {name} process observation {index} escaped its block",
        )
        snapshot = observation["snapshot"]
        if snapshot is None:
            require(
                all(
                    observation[field] is None
                    for field in observation_fields
                    - {"phase", "observed_ns", "snapshot"}
                ),
                f"qualification artifact {name} absent observation retained an identity",
            )
            continue
        validate_snapshot(
            snapshot,
            f"qualification artifact {name} process observation {index}.snapshot",
        )
        for field, expected in contracts.items():
            require(
                observation[field] == expected,
                f"qualification artifact {name} process observation {index}.{field} is stale",
            )
        require(
            observation["server_instance_id"] == snapshot["process"]["server_instance_id"]
            and observation["pid"] == snapshot["process"]["pid"]
            and observation["process_start_id"] == snapshot["process"]["process_start_id"]
            and observation["executable_sha256"] == snapshot["process"]["executable_sha256"]
            and observation["executable_version"] == snapshot["process"]["executable_version"]
            and observation["endpoint_namespace_id"]
            == snapshot["authority"]["endpoint_namespace_id"]
            and observation["lifetime_authority_id"]
            == snapshot["authority"]["lifetime_authority_id"]
            and observation["listener_id"] == snapshot["authority"]["listener_id"],
            f"qualification artifact {name} process observation {index} identity disagrees with its snapshot",
        )
        engine = snapshot.get("engine")
        require(
            observation["load_generation"]
            == (engine.get("load_generation") if isinstance(engine, dict) else None),
            f"qualification artifact {name} process observation {index} load generation disagrees with its snapshot",
        )

    observations = payload["observations"]
    require(
        isinstance(observations, list),
        f"qualification artifact {name} observations are malformed",
    )
    observations_by_kind: dict[str, list[dict]] = {}
    for index, observation in enumerate(observations):
        require(
            isinstance(observation, dict),
            f"qualification artifact {name} observation {index} is malformed",
        )
        require_exact_keys(
            observation,
            {"sequence", "kind", "observed_ns", "values"},
            f"qualification artifact {name} observation {index}",
        )
        require(
            observation["sequence"] == index,
            f"qualification artifact {name} observation sequence is not contiguous",
        )
        kind = require_nonempty_string(
            observation["kind"],
            f"qualification artifact {name} observation {index}.kind",
        )
        observed_ns = require_nonnegative_int(
            observation["observed_ns"],
            f"qualification artifact {name} observation {index}.observed_ns",
        )
        require(
            started_ns <= observed_ns <= finished_ns,
            f"qualification artifact {name} observation {index} escaped its block",
        )
        require(
            isinstance(observation["values"], dict),
            f"qualification artifact {name} observation {index}.values is malformed",
        )
        observations_by_kind.setdefault(kind, []).append(observation)

    required_transitions = {
        "client_death": {
            "dead_client_work_observed",
            "other_client_continued",
            "client_terminated",
            "dead_client_work_reclaimed",
            "post_reclaim_other_client_query",
        },
        "cold_race": {"two_independent_processes", "single_server_convergence"},
        "frozen_owner": {"bounded_owner_unresponsive", "owner_identity_stable"},
        "incompatible_owner": {
            "active_owner_rejected",
            "idle_owner_draining",
            "compatible_replacement",
        },
        "mixed_queue": {
            "queues_saturated",
            "query_selected_before_bulk_backlog",
            "typed_capacity_retry_observed",
            "per_class_fifo_observed",
            "global_fifo_across_projects",
            "query_preference_observed",
            "bulk_resumed",
        },
        "server_crash": {
            "inflight_request_observed",
            "server_replaced",
            "query_replayed",
        },
        "true_idle_respawn": {
            "anti_idle_work_observed",
            "owner_preserved_across_idle_boundary",
            "anti_idle_work_reclaimed",
            "true_idle_wait",
            "idle_surfaces_exercised",
            "owner_absent_after_true_idle",
            "server_respawned",
        },
        "worker_stall": {
            "stalled_request_observed",
            "watchdog_fail_stop_observed",
            "unrelated_process_survived",
            "post_stall_replacement",
        },
    }[scenario_id]
    require(
        all(len(observations_by_kind.get(kind, [])) == 1 for kind in required_transitions),
        f"qualification artifact {name} omitted or duplicated required raw transitions",
    )
    required_controls = {
        "client_death": Counter({"hold_class": 2, "release_class": 2}),
        "cold_race": Counter(),
        "frozen_owner": Counter({"freeze_owner": 1, "release_owner": 1}),
        "incompatible_owner": Counter(
            {"force_incompatible": 1, "clear_incompatible": 1}
        ),
        "mixed_queue": Counter({"hold_class": 2, "release_class": 2}),
        "server_crash": Counter({"hold_class": 1, "crash_server": 1}),
        "true_idle_respawn": Counter({"hold_class": 2, "release_class": 2}),
        "worker_stall": Counter({"stall_native": 1, "release_native": 1}),
    }[scenario_id]
    actual_controls = Counter(control_actions)
    require(
        all(actual_controls[action] >= count for action, count in required_controls.items()),
        f"qualification artifact {name} omitted required authenticated controls",
    )
    if scenario_id == "cold_race":
        require(
            any(
                observation["phase"] == "cold_race_no_owner"
                and observation["snapshot"] is None
                for observation in process_observations
            ),
            f"qualification artifact {name} did not prove owner absence before the race",
        )
        independent = observations_by_kind["two_independent_processes"][0]["values"]
        require(
            independent.get("first_pid") != independent.get("second_pid")
            and independent.get("first_project_identity_sha256")
            != independent.get("second_project_identity_sha256")
            and independent.get("first_transport_peer_verified") is True
            and independent.get("second_transport_peer_verified") is True,
            f"qualification artifact {name} cold-race processes were not independent",
        )

    events = payload["events"]
    require(isinstance(events, list) and events, f"qualification artifact {name} has no correlated events")
    for index, event in enumerate(events):
        require(isinstance(event, dict), f"qualification artifact {name} event {index} is malformed")
        require_exact_keys(
            event,
            {"sequence", "source", "action", "observed_ns", "correlation_id", "values"},
            f"qualification artifact {name} event {index}",
        )
        require(
            event["sequence"] == index,
            f"qualification artifact {name} event sequence is not contiguous",
        )
        require_nonempty_string(
            event["source"],
            f"qualification artifact {name} event {index}.source",
        )
        require_nonempty_string(event["action"], f"qualification artifact {name} event {index}.action")
        observed_ns = require_nonnegative_int(
            event["observed_ns"],
            f"qualification artifact {name} event {index}.observed_ns",
        )
        require(
            started_ns <= observed_ns <= finished_ns,
            f"qualification artifact {name} event {index} escaped its block",
        )
        require(
            event["correlation_id"] is None
            or (
                isinstance(event["correlation_id"], str)
                and bool(event["correlation_id"])
            ),
            f"qualification artifact {name} event {index}.correlation_id is malformed",
        )
        require(
            isinstance(event["values"], dict),
            f"qualification artifact {name} event {index}.values is malformed",
        )

    expected_counts = {
        "process_count": len(invocations),
        "control_event_count": len(control_events),
        "process_observation_count": len(process_observations),
        "observation_count": len(observations),
        "event_count": len(events),
    }
    for field, expected in expected_counts.items():
        require(
            summary[field] == expected,
            f"qualification scenario {scenario_id} summary {field} is stale",
        )
    assertions = derive_scenario_assertions(
        scenario_id,
        observations_by_kind=observations_by_kind,
        process_observations=process_observations,
        invocations=invocations,
        control_actions=control_actions,
        same_account=same_account,
        materialization=materialization,
    )
    return (
        {"name": name, "sha256": hashlib.sha256(payload_bytes).hexdigest()},
        assertions,
    )


def require_candidate_matrix_installation_source(
    cell_id: str | None,
    installation_source: str,
) -> None:
    alias = CANDIDATE_QUALIFICATION_MATRIX_ALIASES.get(cell_id)
    if alias is not None:
        require(
            installation_source == alias["installation_source"],
            "candidate qualification matrix alias requires candidate-installed provenance",
        )


def qualification_measurement_artifact(
    artifact_root: Path,
    summary: object,
    *,
    contracts: dict,
    measurement_contract: dict,
    target: str,
    proof_tier: str,
    matrix_cell_id: str,
    expected_policy: str,
    expected_backend: str,
    forbidden_values: list[str],
) -> dict:
    require(isinstance(summary, dict), "qualification measurement summary is malformed")
    require_exact_keys(
        summary,
        {"artifact", "metric_count", "sample_count"},
        "qualification measurement summary",
    )
    name = require_nonempty_string(summary["artifact"], "qualification measurement artifact")
    relative = Path(name)
    require(
        not relative.is_absolute()
        and len(relative.parts) == 1
        and relative.name == name
        and name == "measurements.raw.json",
        "qualification measurement artifact must be measurements.raw.json",
    )
    path = artifact_root / relative
    require(
        path.is_file() and not path.is_symlink() and path.resolve().parent == artifact_root.resolve(),
        "qualification measurement artifact is missing or unsafe",
    )
    payload_bytes = path.read_bytes()
    for forbidden in forbidden_values:
        require(
            forbidden.encode("utf-8") not in payload_bytes,
            "qualification measurement artifact leaked private request material",
        )
    try:
        payload = json.loads(payload_bytes)
    except json.JSONDecodeError as exc:
        raise ProofFailure(
            f"qualification measurement artifact is not valid JSON: {exc}"
        ) from exc
    require(isinstance(payload, dict), "qualification measurement artifact must be an object")
    require_exact_keys(
        payload,
        {"schema_version", "contracts", "external_metrics", "metrics"},
        "qualification measurement artifact",
    )
    require(payload["schema_version"] == 2, "qualification measurement schema is unsupported")
    require(payload["contracts"] == contracts, "qualification measurements used stale contracts")
    require(
        payload["external_metrics"] == sorted(EXTERNAL_QUALIFICATION_METRICS),
        "qualification measurements changed the externally owned metric set",
    )

    protocol = measurement_contract["measurement_protocol"]
    metric_contracts = protocol["metric_contracts"]
    phase_boundaries = protocol["phase_boundaries"]
    raw_metric_names = set(protocol["required_metrics"]) - EXTERNAL_QUALIFICATION_METRICS
    matrix_cell = selected_qualification_matrix_cell(
        protocol,
        cell_id=matrix_cell_id,
        target=target,
        proof_tier=proof_tier,
        expected_policy=expected_policy,
        expected_backend=expected_backend,
    )
    metrics = payload["metrics"]
    require(
        isinstance(metrics, dict) and set(metrics) == raw_metric_names,
        "qualification measurements did not contain exactly the 12 product-path metrics",
    )
    require(
        summary["metric_count"] == len(raw_metric_names),
        "qualification measurement metric count is stale",
    )
    target_os = TARGET_CONTRACTS[target]["target_os"]
    clock_policy = protocol["clock_policy"]
    allowed_awake_apis = set(clock_policy["platform_apis"][target_os])
    suspend_contract = clock_policy["suspend_detection"]
    inclusive_api = suspend_contract["platform_apis"][target_os]
    maximum_suspend_ns = require_nonnegative_int(
        suspend_contract["maximum_inclusive_minus_awake_ns"],
        "measurement suspend-detection tolerance",
    )
    duration_metrics = raw_metric_names - {
        "bulk_documents_per_second",
        "bulk_tokens_per_second",
        "total_codestory_process_memory",
        "backend_observed_accelerator_residency",
    }
    values: dict[str, float | int] = {}
    sample_count = 0
    for metric in sorted(raw_metric_names):
        record = metrics[metric]
        require(isinstance(record, dict), f"qualification measurement {metric} is malformed")
        require_exact_keys(record, {"unit", "samples"}, f"qualification measurement {metric}")
        require(
            record["unit"] == metric_contracts[metric]["unit"],
            f"qualification measurement {metric} used the wrong unit",
        )
        samples = record["samples"]
        sample_policy = protocol["metric_sampling"][metric]
        require(
            isinstance(samples, list)
            and len(samples) == sample_policy["sample_count"],
            f"qualification measurement {metric} sample count changed",
        )
        sample_count += len(samples)
        sample_values: list[float | int] = []
        sample_ids: set[str] = set()
        server_identities: list[tuple[str, str, int]] = []
        for sample_index, sample in enumerate(samples):
            require(
                isinstance(sample, dict),
                f"qualification measurement {metric} sample {sample_index} is malformed",
            )
            require_exact_keys(
                sample,
                {
                    "sample_id",
                    "repeat",
                    "matrix_cell_id",
                    "workload_id",
                    "cache_state",
                    "residency_state",
                    "process",
                    "server_identity",
                    "clock",
                    "start",
                    "end",
                    "operands",
                    "suspend_witness",
                },
                f"qualification measurement {metric} sample {sample_index}",
            )
            sample_id = require_opaque_identifier(
                sample["sample_id"],
                f"qualification measurement {metric} sample_id",
            )
            require(
                sample_id not in sample_ids,
                f"qualification measurement {metric} duplicated a sample id",
            )
            sample_ids.add(sample_id)
            require(
                sample["repeat"] == sample_index + 1,
                f"qualification measurement {metric} repeat sequence is not exact",
            )
            require(
                sample["matrix_cell_id"] == matrix_cell_id,
                f"qualification measurement {metric} used the wrong host/package matrix cell",
            )
            require(
                sample["workload_id"] == protocol["workloads"][metric]["workload_id"],
                f"qualification measurement {metric} used the wrong workload",
            )
            require(
                sample["cache_state"] == matrix_cell["cache_state"]
                and sample["residency_state"] == matrix_cell["residency_state"],
                f"qualification measurement {metric} changed cache or residency state",
            )
            server_identity = sample["server_identity"]
            require(
                isinstance(server_identity, dict),
                f"qualification measurement {metric} server identity is malformed",
            )
            require_exact_keys(
                server_identity,
                {"server_instance_id", "process_start_id", "load_generation"},
                f"qualification measurement {metric} server identity",
            )
            server_identities.append(
                (
                    require_opaque_identifier(
                        server_identity["server_instance_id"],
                        f"qualification measurement {metric} server_instance_id",
                    ),
                    require_nonempty_string(
                        server_identity["process_start_id"],
                        f"qualification measurement {metric} server process_start_id",
                    ),
                    require_positive_int(
                        server_identity["load_generation"],
                        f"qualification measurement {metric} server load_generation",
                    ),
                )
            )
            sample_values.append(
                qualification_measurement_sample_value(
                    metric,
                    sample,
                    contracts=contracts,
                    phase_boundaries=phase_boundaries,
                    allowed_awake_apis=allowed_awake_apis,
                    inclusive_api=inclusive_api,
                    maximum_suspend_ns=maximum_suspend_ns,
                    expected_policy=expected_policy,
                    expected_backend=expected_backend,
                )
            )
        if sample_policy.get("independence") == "distinct_server_instance_per_sample":
            require(
                len({identity[:2] for identity in server_identities}) == len(samples),
                f"qualification measurement {metric} repeats did not use distinct server instances",
            )
        else:
            require(
                len(set(server_identities)) == 1,
                f"qualification measurement {metric} changed server identity within its repeated block",
            )
        aggregation = sample_policy["aggregation"]
        values[metric] = {
            "maximum": max,
            "minimum": min,
            "exact": lambda raw: raw[0],
        }[aggregation](sample_values)

    require(
        summary["sample_count"] == sample_count,
        "qualification measurement sample count is stale",
    )
    return {
        "artifact": {
            "name": name,
            "sha256": hashlib.sha256(payload_bytes).hexdigest(),
        },
        "values": values,
        "unplanned_suspend": False,
        "matrix_cell_id": matrix_cell_id,
        "payload": payload,
    }
