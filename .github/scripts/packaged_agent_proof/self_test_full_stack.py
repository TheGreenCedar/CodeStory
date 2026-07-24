"""Self Test for packaged CodeStory proof."""

from .foundation import *
from .contracts import (
    ProofFailure,
    assert_retained_json_privacy,
    canonical_sha256,
    load_holdout_task_contracts,
    require,
    require_sha256,
    selected_qualification_matrix_cell,
    sha256,
    validate_runtime_claim_scope,
    verify_package_server_contracts,
    write_json,
)
from .archive import (
    embedding_contract_digest,
    expected_archive_digest,
    find_cli,
    load_native_manifest,
    parse_server_proof_identity,
    unpack_archive,
    verify_runtime_against_manifest,
)
from .process import (
    ExactProcessExitWaiter,
    FailurePreservingTemporaryDirectory,
    McpProcess,
    engine_identity,
    extract_resource,
    live_process_executable_sha256,
    native_server_exit_wait_budget,
    native_server_exit_wait_required,
    parse_byte_quantity,
    process_start_identity,
    remaining_native_server_exit_wait_ms,
    require_native_process_start_identity,
    retained_final_native_server_exit_evidence,
    run,
    server_snapshot,
    shared_server_identity,
    verified_live_executable,
)
from .installation import (
    run_parallel,
)
from .runtime import (
    publication_identity_from_status,
    verify_fault_recovery_consistency_raw_evidence,
    verify_publication_fault_raw_evidence,
    verify_retrieval_quality_raw_evidence,
)
from .qualification import (
    derive_scenario_assertions,
    require_candidate_matrix_installation_source,
    verify_retained_qualification,
)
from .calibration import (
    assemble_calibration_bundle,
    build_calibration_self_test_bundle,
    verify_calibration_bundle,
)

def run_full_stack_self_tests() -> None:
    with tempfile.TemporaryDirectory() as raw:
        root = Path(raw)
        self_protocol = root / SERVER_PROTOCOL.name
        self_protocol.write_bytes(SERVER_PROTOCOL.read_bytes())
        self_constant_set = root / SERVER_CONSTANT_SET.name
        unfrozen_constant_set = json.loads(
            SERVER_CONSTANT_SET.read_text(encoding="utf-8")
        )
        unfrozen_constant_set["status"] = "unfrozen"
        unfrozen_constant_set["calibration_required_values"] = {
            field: None
            for field in unfrozen_constant_set["calibration_required_values"]
        }
        unfrozen_constant_set["qualification_thresholds"] = {
            field: None
            for field in unfrozen_constant_set["qualification_thresholds"]
        }
        unfrozen_constant_set["freeze_record"] = None
        write_json(self_constant_set, unfrozen_constant_set)
        self_measurement_protocol = root / MEASUREMENT_PROTOCOL.name
        self_measurement_protocol.write_bytes(MEASUREMENT_PROTOCOL.read_bytes())
        retained_privacy_path = root / "retained-privacy.json"
        write_json(
            retained_privacy_path,
            {"account_id": "account:" + "a" * 64, "transcript_sha256": "b" * 64},
        )
        assert_retained_json_privacy(retained_privacy_path, [str(root), "private query"])
        write_json(retained_privacy_path, {"request_text": "private query"})
        try:
            assert_retained_json_privacy(retained_privacy_path, [str(root), "private query"])
        except ProofFailure:
            pass
        else:
            raise ProofFailure("retained evidence private request text was accepted")
        payload = root / "artifact.zip"
        binary_header = bytearray(64)
        binary_header[:4] = b"\xcf\xfa\xed\xfe"
        struct.pack_into("<I", binary_header, 4, 0x0100000C)
        model = {
            "file_name": "test.gguf",
            "size_bytes": 4,
            "sha256": "a" * 64,
            "embedded": True,
            "producer": {"name": "test", "version": "0.0.0"},
        }
        embedding = {
            "family": "inprocess:test",
            "dimension": 768,
            "query_prefix": "query: ",
            "document_prefix": "",
            "pooling": "cls",
            "normalization": "l2",
            "element_type": "f32_le",
            "vector_schema_version": 2,
        }
        tokenizer = {
            "container": "gguf",
            "tokenizer_sha256": "b" * 64,
            "config_sha256": "c" * 64,
        }
        contract_sha256 = embedding_contract_digest(model, embedding, tokenizer)
        build_identity = (
            "codestory-native-engine-v1|target=aarch64-apple-darwin|os=macos|"
            "arch=aarch64|linkage=static|backend_loading=builtin|"
            "backends=cpu,metal|llama_cpp_crate=test|"
            f"llama_cpp_commit=test|model_sha256={'a' * 64}|"
            f"embedding_contract_sha256={contract_sha256}|model_embedded=true|"
            "producer=test@0.0.0|end"
        )
        protocol_sha256 = sha256(self_protocol)
        constant_set_sha256 = sha256(self_constant_set)
        measurement_protocol_sha256 = sha256(self_measurement_protocol)
        server_proof_identity = (
            "codestory-embedding-server-proof-v1|bootstrap=1|protocol_schema=1|"
            f"protocol_sha256={protocol_sha256}|"
            f"constant_set_sha256={constant_set_sha256}|"
            f"measurement_protocol_sha256={measurement_protocol_sha256}|"
            "clock_policy=awake_monotonic|query_capacity=64|bulk_capacity=64|"
            "idle_timeout_ms=60000|end"
        )
        binary_payload = (
            bytes(binary_header)
            + b"\0"
            + build_identity.encode("ascii")
            + b"\0"
            + server_proof_identity.encode("ascii")
            + b"\0"
        )
        server_proof = parse_server_proof_identity(server_proof_identity)
        server_proof["constant_set_status"] = "unfrozen"
        valid_manifest = {
            "schema_version": 3,
            "release_version": "0.0.0",
            "asset_target": "macos-arm64",
            "source": {
                "commit": "1" * 40,
                "tree": "2" * 40,
                "tracked_dirty": False,
            },
            "binary": {
                "name": "codestory-cli",
                "sha256": hashlib.sha256(binary_payload).hexdigest(),
                "format": "mach-o",
                "arch": "aarch64",
                "needed": [],
            },
            "runtime_artifacts": [],
            "engine": {
                "build_contract_schema_version": 2,
                "build_identity": build_identity,
                "target_triple": "aarch64-apple-darwin",
                "target_os": "macos",
                "target_arch": "aarch64",
                "linkage": "static",
                "backend_loading": "builtin",
                "compiled_backends": ["cpu", "metal"],
                "llama_cpp_crate_version": "test",
                "llama_cpp_source_commit": "test",
                "embedding_contract_sha256": contract_sha256,
            },
            "model": model,
            "embedding": embedding,
            "tokenizer_config": tokenizer,
            "accelerator": {
                "cpu_fallback": "explicit_only",
                "package_claim": "compiled_capability_only",
                "runtime_execution": "not_proven_by_package",
                "expected_protected_backend": "metal",
                "non_claim_reason": None,
            },
            "server_proof": server_proof,
        }
        with zipfile.ZipFile(payload, "w") as handle:
            handle.writestr("codestory-cli", binary_payload)
            handle.writestr(
                NATIVE_MANIFEST_FILE,
                json.dumps(valid_manifest, indent=2, sort_keys=True) + "\n",
            )
        checksum = root / "SHA256SUMS.txt"
        checksum.write_text(f"{sha256(payload)}  {payload.name}\n", encoding="utf-8")
        require(expected_archive_digest(checksum, payload) == sha256(payload), "checksum parser failed")
        unpacked = root / "unpacked"
        unpack_archive(payload, unpacked)
        cli = find_cli(unpacked)
        require(cli.name == "codestory-cli", "CLI discovery failed")
        manifest = load_native_manifest(unpacked, cli, "0.0.0")
        require(manifest["engine"]["linkage"] == "static", "manifest validation failed")
        malicious = root / "malicious.zip"
        with zipfile.ZipFile(malicious, "w") as handle:
            handle.writestr("../outside", b"bad")
        try:
            unpack_archive(malicious, root / "bad")
        except ProofFailure:
            pass
        else:
            raise ProofFailure("archive traversal was accepted")
        valid = {
            "embedding_model_sha256": "a" * 64,
            "embedding_ggml_build_identity": manifest["engine"]["build_identity"],
            "embedding_backend": "Metal",
            "embedding_adapter": "Apple GPU",
            "embedding_policy": "accelerated",
            "embedding_engine_instance_id": "engine-1",
            "embedding_engine_residency": "resident",
            "embedding_engine_load_generation": 1,
            "embedding_model_load_count": 1,
            "embedding_smoke_ms": 1.0,
            "embedding_initialization_ms": 2.0,
            "embedding_accelerator_execution_verified": True,
            "embedding_execution_devices": ["Apple GPU"],
            "embedding_execution_backends": ["Metal"],
            "embedding_execution_observation_source": "ggml_eval_callback",
            "embedding_encode_count": 1,
            "embedding_execution_node_count": 1,
            "embedding_resident_accelerator_tensor_count": 1,
            "embedding_resident_accelerator_tensor_bytes": 1,
            "embedding_model_layer_count": 13,
            "embedding_offloaded_layer_count": 13,
        }
        engine_identity(valid, "accelerated", "Metal")
        runtime = {
            "identity": valid,
            "second_host_identity": valid.copy(),
            "rejoin_identity": valid.copy(),
        }
        evidence = verify_runtime_against_manifest(manifest, runtime, "accelerated")
        require(evidence["execution"] == "proven_by_live_runtime", "runtime contract proof failed")
        snapshot_payload = {
            "embedding_server": {
                "schema_version": 1,
                "event_sequence": 17,
                "lifecycle": "resident",
                "clock": {
                    "domain": "awake_monotonic",
                    "api": "mach_absolute_time",
                    "boot_id": "boot-1",
                    "resolution_ns": 1,
                },
                "protocol": {
                    "bootstrap_version": 1,
                    "schema_version": 1,
                    "protocol_sha256": protocol_sha256,
                    "constant_set_sha256": constant_set_sha256,
                    "measurement_protocol_sha256": measurement_protocol_sha256,
                },
                "authority": {
                    "endpoint_namespace_id": "endpoint-1",
                    "lifetime_authority_id": "authority-1",
                    "listener_id": "listener-1",
                    "peer_verified": True,
                },
                "process": {
                    "server_instance_id": "server-1",
                    "pid": 101,
                    "process_start_id": "boot-1:101",
                    "executable_sha256": manifest["binary"]["sha256"],
                    "executable_version": "0.0.0",
                },
                "scheduler": {
                    "query_capacity": 64,
                    "query_depth": 0,
                    "bulk_capacity": 64,
                    "bulk_depth": 0,
                    "connection_count": 2,
                    "active_request_count": 0,
                    "lease_count": 0,
                    "active_request": None,
                },
                "engine": {
                    "engine_owner_id": "engine-owner-1",
                    "native_worker_id": "native-worker-1",
                    "load_generation": 1,
                    "model_load_count": 1,
                    "successful_encode_count": 3,
                },
                "failure": None,
            }
        }
        first_snapshot = server_snapshot(snapshot_payload, manifest, require_resident=True)
        second_snapshot = server_snapshot(
            json.loads(json.dumps(snapshot_payload)),
            manifest,
            require_resident=True,
        )
        shared = shared_server_identity(first_snapshot, second_snapshot)
        require(shared["model_load_count"] == 1, "shared server identity self-test failed")
        retired_snapshot = json.loads(json.dumps(first_snapshot))
        retired_snapshot["process"]["server_instance_id"] = "retired-server"
        retired_snapshot["authority"]["lifetime_authority_id"] = "retired-authority"
        retired_snapshot["authority"]["listener_id"] = "retired-listener"
        retired_snapshot["engine"]["engine_owner_id"] = "retired-engine-owner"
        retired_snapshot["engine"]["native_worker_id"] = "retired-native-worker"
        elected_snapshot = json.loads(json.dumps(first_snapshot))
        cold_process_observations = [
            {
                "phase": "cold_race_no_owner_before",
                "snapshot": retired_snapshot,
            },
            {"phase": "cold_race_no_owner", "snapshot": None},
            {"phase": "cold_race_first", "snapshot": elected_snapshot},
            {
                "phase": "cold_race_second",
                "snapshot": json.loads(json.dumps(elected_snapshot)),
            },
        ]
        cold_transitions = {
            "two_independent_processes": [
                {
                    "values": {
                        "first_pid": 101,
                        "second_pid": 102,
                        "first_project_identity_sha256": "a" * 64,
                        "second_project_identity_sha256": "b" * 64,
                        "first_transport_peer_verified": True,
                        "second_transport_peer_verified": True,
                    }
                }
            ],
            "single_server_convergence": [
                {
                    "values": {
                        "server_instance_id": elected_snapshot["process"][
                            "server_instance_id"
                        ],
                        "lifetime_authority_id": elected_snapshot["authority"][
                            "lifetime_authority_id"
                        ],
                    }
                }
            ],
        }
        cold_assertions = derive_scenario_assertions(
            "cold_race",
            observations_by_kind=cold_transitions,
            process_observations=cold_process_observations,
            invocations=[],
            control_actions=[],
            same_account={
                "relation": "same_os_account",
                "plugin_hosts": [{"pid": 101}, {"pid": 102}],
            },
            materialization={},
        )
        require(
            all(cold_assertions.values()),
            "retired pre-race owner contaminated post-reset election assertions",
        )
        duplicate_phase_observations = json.loads(
            json.dumps(cold_process_observations)
        )
        duplicate_phase_observations.insert(
            -1, {"phase": "cold_race_first", "snapshot": None}
        )
        try:
            derive_scenario_assertions(
                "cold_race",
                observations_by_kind=cold_transitions,
                process_observations=duplicate_phase_observations,
                invocations=[],
                control_actions=[],
                same_account={
                    "relation": "same_os_account",
                    "plugin_hosts": [{"pid": 101}, {"pid": 102}],
                },
                materialization={},
            )
        except ProofFailure as error:
            require(
                str(error)
                == "cold race must retain exactly one post-reset snapshot from each process",
                "duplicate cold-race phase changed its cardinality failure",
            )
        else:
            raise ProofFailure("duplicate cold-race phase observation was accepted")
        split_process_observations = json.loads(
            json.dumps(cold_process_observations)
        )
        split_snapshot = split_process_observations[-1]["snapshot"]
        split_snapshot["process"]["server_instance_id"] = "split-server"
        split_snapshot["authority"]["lifetime_authority_id"] = "split-authority"
        split_snapshot["authority"]["listener_id"] = "split-listener"
        split_snapshot["engine"]["engine_owner_id"] = "split-engine-owner"
        split_snapshot["engine"]["native_worker_id"] = "split-native-worker"
        expected_split_failures = {
            "one_engine_owner",
            "one_lifetime_authority",
            "one_listener",
            "one_model_load",
            "one_native_worker",
            "one_server",
        }
        try:
            derive_scenario_assertions(
                "cold_race",
                observations_by_kind=cold_transitions,
                process_observations=split_process_observations,
                invocations=[],
                control_actions=[],
                same_account={
                    "relation": "same_os_account",
                    "plugin_hosts": [{"pid": 101}, {"pid": 102}],
                },
                materialization={},
            )
        except ProofFailure as error:
            require(
                str(error)
                == "qualification scenario cold_race raw evidence failed assertions: "
                + ", ".join(sorted(expected_split_failures)),
                "post-reset cold-race split changed its exact identity failures",
            )
        else:
            raise ProofFailure(
                "post-reset cold-race split escaped its exact identity assertions"
            )
        true_idle_before = json.loads(json.dumps(first_snapshot))
        true_idle_respawned = json.loads(json.dumps(first_snapshot))
        true_idle_respawned["process"]["server_instance_id"] = "respawned-server"
        true_idle_respawned["process"]["pid"] = 303
        true_idle_respawned["process"]["process_start_id"] = "process-start-303"
        true_idle_respawned["authority"]["lifetime_authority_id"] = (
            "respawned-authority"
        )
        true_idle_respawned["authority"]["listener_id"] = "respawned-listener"
        true_idle_respawned["engine"]["engine_owner_id"] = "respawned-engine-owner"
        true_idle_respawned["engine"]["native_worker_id"] = "respawned-native-worker"
        true_idle_process_observations = [
            {
                "phase": "true_idle_before",
                "observed_ns": 100,
                "snapshot": true_idle_before,
            },
            {
                "phase": "true_idle_after_wait",
                "observed_ns": 200,
                "snapshot": None,
            },
            {
                "phase": "true_idle_respawned",
                "observed_ns": 400,
                "snapshot": true_idle_respawned,
            },
        ]
        active_scheduler = {
            "query_capacity": 64,
            "query_depth": 1,
            "bulk_capacity": 64,
            "bulk_depth": 1,
            "active_request_count": 1,
            "lease_count": 1,
            "active_request_class": "query",
        }
        reclaimed_scheduler = {
            "query_capacity": 64,
            "query_depth": 0,
            "bulk_capacity": 64,
            "bulk_depth": 0,
            "active_request_count": 0,
            "lease_count": 0,
            "active_request_class": None,
        }
        materialized_sha256 = "c" * 64
        true_idle_transitions = {
            "anti_idle_work_observed": [{"values": active_scheduler}],
            "owner_preserved_across_idle_boundary": [
                {
                    "values": {
                        "held_started_ns": 0,
                        "held_observed_ns": 60_000_000_000,
                        "contract_idle_timeout_ms": 60_000,
                        "server_instance_id": true_idle_before["process"][
                            "server_instance_id"
                        ],
                    }
                }
            ],
            "anti_idle_work_reclaimed": [{"values": reclaimed_scheduler}],
            "true_idle_wait": [
                {
                    "values": {
                        "server_idle_epoch_ns": 1,
                        "server_idle_elapsed_before_client_wait_ns": 59_000_000_000,
                        "client_wait_required_ns": 1_000_000_000,
                        "client_wait_elapsed_ns": 1_000_000_000,
                        "contract_idle_timeout_ms": 60_000,
                        "clock_boot_id": "boot-1",
                    }
                }
            ],
            "idle_surfaces_exercised": [
                {
                    "values": {
                        "diagnostic_count": 2,
                        "idle_connection_close_count": 2,
                        "last_diagnostic_client_elapsed_ns": 30_000_000_000,
                        "last_idle_connection_close_client_elapsed_ns": 30_000_000_000,
                    }
                }
            ],
            "owner_absent_after_true_idle": [
                {
                    "observed_ns": 225,
                    "values": {
                        "old_server_instance_id": true_idle_before["process"][
                            "server_instance_id"
                        ]
                    }
                }
            ],
            "server_respawned": [
                {
                    "observed_ns": 450,
                    "values": {
                        "new_server_instance_id": true_idle_respawned["process"][
                            "server_instance_id"
                        ],
                        "load_generation": 1,
                        "model_load_count": 1,
                        "materialized_model_sha256": materialized_sha256,
                        "materialized_reused": True,
                    }
                }
            ],
        }
        true_idle_invocations = [
            {
                "operation": "query",
                "started_ns": 250,
                "finished_ns": 350,
                "exit_code": 0,
                "termination": "exited",
            },
            {
                "operation": "query",
                "started_ns": 10,
                "finished_ns": 20,
                "exit_code": 0,
                "termination": "exited",
            },
        ]
        true_idle_assertions = derive_scenario_assertions(
            "true_idle_respawn",
            observations_by_kind=true_idle_transitions,
            process_observations=true_idle_process_observations,
            invocations=true_idle_invocations,
            control_actions=[],
            same_account={},
            materialization={
                "sha256": materialized_sha256,
                "reused_on_rejoin": False,
            },
        )
        require(
            all(true_idle_assertions.values()),
            "cold first-use state contaminated replacement materialization proof",
        )
        for field, hostile_value in (
            ("materialized_reused", False),
            ("materialized_model_sha256", "d" * 64),
        ):
            hostile_transitions = json.loads(json.dumps(true_idle_transitions))
            hostile_transitions["server_respawned"][0]["values"][field] = hostile_value
            try:
                derive_scenario_assertions(
                    "true_idle_respawn",
                    observations_by_kind=hostile_transitions,
                    process_observations=true_idle_process_observations,
                    invocations=true_idle_invocations,
                    control_actions=[],
                    same_account={},
                    materialization={
                        "sha256": materialized_sha256,
                        "reused_on_rejoin": False,
                    },
                )
            except ProofFailure as error:
                require(
                    str(error)
                    == "qualification scenario true_idle_respawn raw evidence failed assertions: verified_materialization_reused",
                    f"hostile true-idle {field} changed its exact failure",
                )
            else:
                raise ProofFailure(
                    f"hostile true-idle {field} escaped replacement binding"
                )
        historical_only_invocations = json.loads(json.dumps(true_idle_invocations[1:]))
        try:
            derive_scenario_assertions(
                "true_idle_respawn",
                observations_by_kind=true_idle_transitions,
                process_observations=true_idle_process_observations,
                invocations=historical_only_invocations,
                control_actions=[],
                same_account={},
                materialization={
                    "sha256": materialized_sha256,
                    "reused_on_rejoin": False,
                },
            )
        except ProofFailure as error:
            require(
                str(error)
                == "qualification scenario true_idle_respawn raw evidence failed assertions: next_product_operation_respawns_without_consent",
                "historical true-idle query changed its exact temporal failure",
            )
        else:
            raise ProofFailure("historical query satisfied true-idle respawn proof")
        failed_then_successful_invocations = [
            {
                "operation": "query",
                "started_ns": 230,
                "finished_ns": 240,
                "exit_code": 1,
                "termination": "exited",
            },
            *true_idle_invocations,
        ]
        try:
            derive_scenario_assertions(
                "true_idle_respawn",
                observations_by_kind=true_idle_transitions,
                process_observations=true_idle_process_observations,
                invocations=failed_then_successful_invocations,
                control_actions=[],
                same_account={},
                materialization={
                    "sha256": materialized_sha256,
                    "reused_on_rejoin": False,
                },
            )
        except ProofFailure as error:
            require(
                str(error)
                == "qualification scenario true_idle_respawn raw evidence failed assertions: next_product_operation_respawns_without_consent",
                "failed first true-idle query changed its exact failure",
            )
        else:
            raise ProofFailure("failed first query was hidden by a later respawn success")
        historical_respawn_transition = json.loads(json.dumps(true_idle_transitions))
        historical_respawn_transition["server_respawned"][0]["observed_ns"] = 150
        try:
            derive_scenario_assertions(
                "true_idle_respawn",
                observations_by_kind=historical_respawn_transition,
                process_observations=true_idle_process_observations,
                invocations=true_idle_invocations,
                control_actions=[],
                same_account={},
                materialization={
                    "sha256": materialized_sha256,
                    "reused_on_rejoin": False,
                },
            )
        except ProofFailure as error:
            require(
                str(error)
                == "qualification scenario true_idle_respawn raw evidence failed assertions: next_product_operation_respawns_without_consent",
                "historical true-idle respawn transition changed its temporal failure",
            )
        else:
            raise ProofFailure("historical respawn transition was accepted")
        duplicate_absence = json.loads(json.dumps(true_idle_process_observations))
        duplicate_absence.insert(
            -1,
            {
                "phase": "true_idle_after_wait",
                "observed_ns": 225,
                "snapshot": None,
            },
        )
        try:
            derive_scenario_assertions(
                "true_idle_respawn",
                observations_by_kind=true_idle_transitions,
                process_observations=duplicate_absence,
                invocations=true_idle_invocations,
                control_actions=[],
                same_account={},
                materialization={
                    "sha256": materialized_sha256,
                    "reused_on_rejoin": False,
                },
            )
        except ProofFailure as error:
            require(
                str(error) == "true idle must retain exactly one absent-owner witness",
                "duplicate true-idle absence changed its cardinality failure",
            )
        else:
            raise ProofFailure("duplicate true-idle absence witness was accepted")
        measurement_contract = verify_package_server_contracts(
            manifest,
            self_measurement_protocol,
            require_frozen=False,
        )
        windows_candidate_cell_id = "candidate_installed_windows_x64_cpu"
        windows_candidate_cell = selected_qualification_matrix_cell(
            measurement_contract["measurement_protocol"],
            cell_id=windows_candidate_cell_id,
            target="windows-x64",
            proof_tier="installed_runtime",
            expected_policy="cpu_explicit",
            expected_backend="CPU",
        )
        require(
            windows_candidate_cell
            == {
                "asset_target": "windows-x64",
                "proof_tier": "installed_runtime",
                "host_class": "premerge_candidate_windows_x64",
                "policy": "cpu_explicit",
                "backend": "cpu",
                "cache_state": "reused",
                "residency_state": "resident",
                "accelerator_claim": "none",
            },
            "Windows candidate-installed alias changed its exact identity",
        )
        require_candidate_matrix_installation_source(
            windows_candidate_cell_id,
            "candidate",
        )
        try:
            require_candidate_matrix_installation_source(
                windows_candidate_cell_id,
                "marketplace",
            )
        except ProofFailure:
            pass
        else:
            raise ProofFailure(
                "Windows candidate-installed alias accepted marketplace provenance"
            )
        hostile_windows_alias_values = {
            "asset_target": "linux-x64",
            "proof_tier": "protected_hardware",
            "policy": "accelerated",
            "backend": "vulkan",
            "accelerator_claim": "vulkan",
        }
        for field, hostile_value in hostile_windows_alias_values.items():
            hostile_protocol = json.loads(
                json.dumps(measurement_contract["measurement_protocol"])
            )
            hostile_protocol["host_package_matrix"][
                "installed_windows_x64_cpu"
            ][field] = hostile_value
            try:
                selected_qualification_matrix_cell(
                    hostile_protocol,
                    cell_id=windows_candidate_cell_id,
                    target="windows-x64",
                    proof_tier="installed_runtime",
                    expected_policy="cpu_explicit",
                    expected_backend="CPU",
                )
            except ProofFailure:
                pass
            else:
                raise ProofFailure(
                    f"Windows candidate-installed alias accepted changed {field}"
                )
        (
            calibration_bundle_path,
            frozen_measurement_contract,
            calibration_bundle_payload,
        ) = build_calibration_self_test_bundle(
            root,
            measurement_contract,
            source=manifest["source"],
        )
        assembled_run_paths = []
        for index, run in enumerate(calibration_bundle_payload["runs"]):
            run_path = root / "assembler-runs" / f"run-{index + 1}.json"
            write_json(run_path, run)
            assembled_run_paths.append(run_path)
        assembled_bundle_path = root / "assembled-calibration-bundle.json"
        assembled_constant_path = root / "assembled-constant-set.json"
        assembled = assemble_calibration_bundle(
            argparse.Namespace(
                measurement_protocol=self_measurement_protocol,
                calibration_bundle_output=assembled_bundle_path,
                frozen_constant_set_output=assembled_constant_path,
                freeze_selected_at="self-test",
                calibration_run=assembled_run_paths,
                calibration_producer_repository="TheGreenCedar/CodeStory",
                calibration_producer_workflow_path=(
                    ".github/workflows/packaged-platform-pr.yml"
                ),
                calibration_producer_run_id="123",
                calibration_producer_run_attempt="1",
                calibration_producer_artifact=(
                    f"embedding-calibration-bundle-{manifest['source']['commit']}"
                ),
            )
        )
        require(
            assembled["run_count"] == 6
            and assembled["matrix_cell_count"] == 2
            and assembled_bundle_path.is_file()
            and assembled_constant_path.is_file(),
            "calibration assembler did not produce the exact frozen artifacts",
        )
        calibration_result = verify_calibration_bundle(
            calibration_bundle_path,
            frozen_measurement_contract,
            enforce_source_lineage=False,
        )
        require(
            calibration_result["run_count"] == 6
            and calibration_result["matrix_cell_count"] == 2,
            "calibration bundle self-test did not verify the full matrix",
        )
        hostile_calibration = json.loads(json.dumps(calibration_bundle_payload))
        hostile_calibration["runs"].pop()
        hostile_calibration_path = root / "hostile-calibration-bundle.json"
        write_json(hostile_calibration_path, hostile_calibration)
        try:
            verify_calibration_bundle(
                hostile_calibration_path,
                frozen_measurement_contract,
                enforce_source_lineage=False,
            )
        except ProofFailure:
            pass
        else:
            raise ProofFailure("incomplete calibration matrix was accepted")
        hostile_calibration = json.loads(json.dumps(calibration_bundle_payload))
        hostile_run = hostile_calibration["runs"][0]
        hostile_metric = hostile_run["raw_artifact"]["payload"]["metrics"][
            "cold_first_vector"
        ]
        hostile_metric["samples"][0]["operands"].pop(
            "successful_operation_duration_ns"
        )
        hostile_run["raw_artifact"]["sha256"] = canonical_sha256(
            hostile_run["raw_artifact"]["payload"]
        )
        write_json(hostile_calibration_path, hostile_calibration)
        try:
            verify_calibration_bundle(
                hostile_calibration_path,
                frozen_measurement_contract,
                enforce_source_lineage=False,
            )
        except ProofFailure:
            pass
        else:
            raise ProofFailure(
                "calibration sample without successful operation duration was accepted"
            )
        hostile_calibration = json.loads(json.dumps(calibration_bundle_payload))
        first_sample_id = hostile_calibration["runs"][0]["raw_artifact"]["payload"][
            "metrics"
        ]["warm_query_ipc"]["samples"][0]["sample_id"]
        duplicate_run = hostile_calibration["runs"][1]
        duplicate_run["raw_artifact"]["payload"]["metrics"]["warm_query_ipc"][
            "samples"
        ][0]["sample_id"] = first_sample_id
        duplicate_run["raw_artifact"]["sha256"] = canonical_sha256(
            duplicate_run["raw_artifact"]["payload"]
        )
        write_json(hostile_calibration_path, hostile_calibration)
        try:
            verify_calibration_bundle(
                hostile_calibration_path,
                frozen_measurement_contract,
                enforce_source_lineage=False,
            )
        except ProofFailure:
            pass
        else:
            raise ProofFailure("duplicate calibration sample identity was accepted")
        hostile_frozen_contract = json.loads(json.dumps(frozen_measurement_contract))
        hostile_frozen_contract["constant_set"]["qualification_thresholds"][
            "warm_query_ipc"
        ] += 1
        try:
            verify_calibration_bundle(
                calibration_bundle_path,
                hostile_frozen_contract,
                enforce_source_lineage=False,
            )
        except ProofFailure:
            pass
        else:
            raise ProofFailure("post-result calibration threshold change was accepted")
        self_digest = lambda label: hashlib.sha256(label.encode("utf-8")).hexdigest()
        external_package = {
            "archive_sha256": "b" * 64,
            "executable_sha256": manifest["binary"]["sha256"],
            "asset_target": manifest["asset_target"],
            "release_version": manifest["release_version"],
        }
        external_contracts = {
            "protocol_sha256": protocol_sha256,
            "constant_set_sha256": constant_set_sha256,
            "measurement_protocol_sha256": measurement_protocol_sha256,
        }
        correlation_id = "0123456789abcdef0123456789abcdef"
        previous_publication = self_digest("previous-publication")
        publication_payload = {
            "schema_version": 1,
            "evidence_contract": PUBLICATION_FAULT_EVIDENCE_CONTRACT,
            "source": manifest["source"],
            "package": external_package,
            "contracts": external_contracts,
            "correlation_id": correlation_id,
            "previous_publication_identity_sha256": previous_publication,
            "server_observations": [
                {
                    "phase": "before_crash",
                    "server_instance_id": "server-before",
                    "process_start_id": "boot-1:101",
                    "load_generation": 1,
                },
                {
                    "phase": "after_replacement",
                    "server_instance_id": "server-after",
                    "process_start_id": "boot-1:102",
                    "load_generation": 1,
                },
            ],
            "candidate_observation": {
                "command": "retrieval_index",
                "exit_code": 1,
                "stdout_sha256": self_digest("candidate-stdout"),
                "stderr_sha256": self_digest("candidate-stderr"),
            },
            "publication_hook_events": [
                {
                    "schema_version": 1,
                    "sequence": index,
                    "correlation_id": correlation_id,
                    "action": action,
                    "status": status,
                    "clock": {
                        "domain": "process_monotonic",
                        "api": "std::time::Instant",
                        "elapsed_ns": index,
                    },
                }
                for index, (action, status) in enumerate(
                    (
                        ("pause_before_manifest_commit", "waiting_for_resume"),
                        ("resume_manifest_commit", "observed"),
                        ("lease_revalidation", "failed"),
                        ("manifest_commit", "returned_error"),
                    )
                )
            ],
            "ordinary_product_observations": [
                {
                    "sequence": index,
                    "command": command,
                    "exit_code": 0,
                    "retrieval_mode": "full",
                    "publication_identity_sha256": previous_publication,
                    "output_sha256": self_digest(f"{command}-output"),
                }
                for index, command in enumerate(("retrieval_status", "search"))
            ],
        }
        publication_path = root / "publication-fault.raw.json"
        write_json(publication_path, publication_payload)
        publication_external = verify_publication_fault_raw_evidence(
            publication_path,
            source=manifest["source"],
            package=external_package,
            contracts=external_contracts,
        )
        require(
            all(publication_external["assertions"].values()),
            "publication fault self-test did not derive its assertions",
        )
        hostile_publication = json.loads(json.dumps(publication_payload))
        hostile_publication["assertions"] = {"lost_publication_lease_blocks_commit": True}
        write_json(publication_path, hostile_publication)
        try:
            verify_publication_fault_raw_evidence(
                publication_path,
                source=manifest["source"],
                package=external_package,
                contracts=external_contracts,
            )
        except ProofFailure:
            pass
        else:
            raise ProofFailure("self-declared publication assertions were accepted")
        write_json(publication_path, publication_payload)
        scenario_observations = {
            "inflight_request_observed": [
                {
                    "values": {
                        "query_capacity": 64,
                        "query_depth": 0,
                        "bulk_capacity": 64,
                        "bulk_depth": 0,
                        "active_request_count": 1,
                        "lease_count": 0,
                        "active_request_class": "query",
                    }
                }
            ],
            "server_replaced": [
                {
                    "values": {
                        "old_server_instance_id": "server-before",
                        "new_server_instance_id": "server-after",
                    }
                }
            ],
            "query_replayed": [
                {
                    "values": {
                        "logical_operation_count": 1,
                        "wire_attempt_count": 2,
                        "wire_attempts": [
                            {
                                "ordinal": 1,
                                "request_id": "request-before",
                                "server_instance_id": "server-before",
                                "submitted_ns": 1,
                                "completed_ns": 2,
                                "outcome": "server_loss",
                            },
                            {
                                "ordinal": 2,
                                "request_id": "request-after",
                                "server_instance_id": "server-after",
                                "submitted_ns": 3,
                                "completed_ns": 4,
                                "outcome": "completed",
                            },
                        ],
                    }
                }
            ],
        }
        derived_server_crash = derive_scenario_assertions(
            "server_crash",
            observations_by_kind=scenario_observations,
            process_observations=[],
            invocations=[],
            control_actions=["hold_class", "crash_server"],
            same_account={},
            materialization={},
        )
        require(
            derived_server_crash
            == {
                "one_replacement_server": True,
                "pure_embedding_rpc_replayed_at_most_once": True,
            },
            "scenario assertion self-test did not derive exact raw claims",
        )
        hostile_scenario = json.loads(json.dumps(scenario_observations))
        hostile_scenario["query_replayed"][0]["values"]["wire_attempts"][1][
            "outcome"
        ] = "server_loss"
        try:
            derive_scenario_assertions(
                "server_crash",
                observations_by_kind=hostile_scenario,
                process_observations=[],
                invocations=[],
                control_actions=["hold_class", "crash_server"],
                same_account={},
                materialization={},
            )
        except ProofFailure:
            pass
        else:
            raise ProofFailure("named scenario transitions with false values were accepted")

        consistency_payload = {
            "schema_version": 1,
            "evidence_contract": FAULT_RECOVERY_CONSISTENCY_CONTRACT,
            "source": manifest["source"],
            "package": external_package,
            "contracts": external_contracts,
            "run_id_sha256": self_digest("consistency-run"),
            "observations": [
                {
                    "case_id_sha256": self_digest(f"consistency-case-{index}"),
                    "before_server_fault_rank": 1,
                    "after_server_replacement_rank": 1,
                }
                for index in range(FAULT_RECOVERY_CONSISTENCY_CASES)
            ],
        }
        consistency_path = root / "fault-recovery-consistency.raw.json"
        write_json(consistency_path, consistency_payload)
        consistency_external = verify_fault_recovery_consistency_raw_evidence(
            consistency_path,
            source=manifest["source"],
            package=external_package,
            contracts=external_contracts,
        )
        require(
            consistency_external["ranks_stable"] is True,
            "fault recovery consistency self-test did not derive stable ranks",
        )
        hostile_consistency = json.loads(json.dumps(consistency_payload))
        hostile_consistency["observations"][0]["after_server_replacement_rank"] = 2
        write_json(consistency_path, hostile_consistency)
        try:
            verify_fault_recovery_consistency_raw_evidence(
                consistency_path,
                source=manifest["source"],
                package=external_package,
                contracts=external_contracts,
            )
        except ProofFailure:
            pass
        else:
            raise ProofFailure("changed fault-recovery search ranks were accepted")

        packet_row = {
            "repo": "fixture",
            "task_id": "quality-contract",
            "mode": "cold_cli_packet",
            "status": "pass",
            "quality": {"pass": True},
            "sufficiency": {
                "status": "sufficient",
                "sufficient_quality_mismatch": False,
                "follow_up_commands_count": 0,
                "open_next_count": 0,
                "gaps_count": 0,
                "coverage_unresolved_blocking_count": 0,
            },
            "packet_latency": {
                "sla_missed": False,
                "retrieval_shadow": {"retrieval_mode": "full"},
            },
            "repo_provenance": {
                "manifest_overridden_by_builtin": False,
                "configured": {
                    "url": "https://github.com/example/fixture.git",
                    "ref": "9" * 40,
                },
                "manifest": {
                    "url": "https://github.com/example/fixture.git",
                    "ref": "9" * 40,
                },
                "git_head": "9" * 40,
                "git_origin": "https://github.com/example/fixture.git",
                "git_dirty": False,
            },
            "codestory_cache_provenance": {
                "doctor_status": "pass",
                "storage_path": "fixture/codestory.db",
                "cache_policy": "prepared-retrieval-cache-read-only",
                "retrieval_mode": "full",
                "semantic_generation": "fixture-generation",
                "manifest_embedding_backend": "per-user-server:coderank-embed:q8_0",
                "semantic_backend": "coderank-embed",
                "embedding_engine_instance_id": "engine-fixture",
                "embedding_policy": "accelerated",
                "local_only": True,
                "indexed": True,
                "freshness_status": "fresh",
                "semantic_ready": True,
                "indexing_in_timed_run": False,
            },
        }
        holdout_tasks, _holdout_manifest_set_sha256 = load_holdout_task_contracts()
        quality_rows = []
        for (repo_name, task_id), task_contract in sorted(holdout_tasks.items()):
            for repeat in range(1, MIN_RETRIEVAL_QUALITY_REPEATS + 1):
                row = json.loads(json.dumps(packet_row))
                row["repo"] = repo_name
                row["task_id"] = task_id
                row["repeat"] = repeat
                row["task_manifest_snapshot"] = {
                    **task_contract["snapshot"],
                    "manifest_path": str(task_contract["path"]),
                }
                row["repo_provenance"]["configured"] = {
                    "url": task_contract["repo"]["url"],
                    "ref": task_contract["repo"]["ref"],
                    "languages": task_contract["repo"].get("languages", []),
                }
                row["repo_provenance"]["manifest"] = {
                    "url": task_contract["repo"]["url"],
                    "ref": task_contract["repo"]["ref"],
                    "workspace_root": task_contract["repo"].get("workspace_root"),
                }
                row["repo_provenance"]["git_head"] = task_contract["repo"]["ref"]
                row["repo_provenance"]["git_origin"] = task_contract["repo"]["url"]
                quality_rows.append(row)
        quality_payload = {
            "modes": ["cold-cli"],
            "repeats": MIN_RETRIEVAL_QUALITY_REPEATS,
            "release_evidence": {
                "commit": manifest["source"]["commit"],
                "source_tree": manifest["source"]["tree"],
                "evaluation_contract": RETRIEVAL_QUALITY_EVIDENCE_CONTRACT,
                "profile": "self-test",
                "evidence_identity": {
                    "corpus_id": RELEASE_QUALITY_CORPUS_ID,
                    "cache_id": "self-test-cache",
                    "machine_fingerprint": "self-test-host",
                },
                "publishable": True,
                "repeats": MIN_RETRIEVAL_QUALITY_REPEATS,
                "quality_gate_status": "pass",
                "publishable_blockers": [],
                "rows": quality_rows,
            },
        }
        quality_path = root / "packet-runtime-summary.json"
        write_json(quality_path, quality_payload)
        quality_external = verify_retrieval_quality_raw_evidence(
            quality_path,
            source=manifest["source"],
        )
        require(
            quality_external["publishable_packet_pass_rate"] == 1,
            "retrieval quality self-test did not derive the packet pass rate",
        )
        hostile_quality = json.loads(json.dumps(quality_payload))
        hostile_quality["assertions"] = {"retrieval_quality": True}
        write_json(quality_path, hostile_quality)
        try:
            verify_retrieval_quality_raw_evidence(
                quality_path,
                source=manifest["source"],
            )
        except ProofFailure:
            pass
        else:
            raise ProofFailure("self-declared retrieval quality pass was accepted")
        hostile_quality = json.loads(json.dumps(quality_payload))
        hostile_quality["release_evidence"]["rows"].pop()
        write_json(quality_path, hostile_quality)
        try:
            verify_retrieval_quality_raw_evidence(
                quality_path,
                source=manifest["source"],
            )
        except ProofFailure:
            pass
        else:
            raise ProofFailure("incomplete retrieval quality repeats were accepted")
        hostile_quality = json.loads(json.dumps(quality_payload))
        hostile_quality["release_evidence"]["rows"] = [
            row
            for row in hostile_quality["release_evidence"]["rows"]
            if row["task_id"] == "axios-request-dispatch"
        ]
        write_json(quality_path, hostile_quality)
        try:
            verify_retrieval_quality_raw_evidence(
                quality_path,
                source=manifest["source"],
            )
        except ProofFailure:
            pass
        else:
            raise ProofFailure("one-task retrieval quality subset was accepted")
        hostile_quality = json.loads(json.dumps(quality_payload))
        hostile_quality["release_evidence"]["rows"][0]["task_manifest_snapshot"][
            "prompt"
        ] = "hostile substituted task"
        write_json(quality_path, hostile_quality)
        try:
            verify_retrieval_quality_raw_evidence(
                quality_path,
                source=manifest["source"],
            )
        except ProofFailure:
            pass
        else:
            raise ProofFailure("substituted retrieval quality task manifest was accepted")
        hostile_quality = json.loads(json.dumps(quality_payload))
        hostile_quality["release_evidence"]["source_tree"] = "f" * 40
        write_json(quality_path, hostile_quality)
        try:
            verify_retrieval_quality_raw_evidence(
                quality_path,
                source=manifest["source"],
            )
        except ProofFailure:
            pass
        else:
            raise ProofFailure("stale retrieval quality source tree was accepted")
        write_json(quality_path, quality_payload)
        try:
            verify_package_server_contracts(
                manifest,
                self_measurement_protocol,
                require_frozen=True,
            )
        except ProofFailure:
            pass
        else:
            raise ProofFailure("unfrozen server constants were accepted for qualification")
        scenario_contracts = measurement_contract["measurement_protocol"]["scenario_contracts"]
        qualification_contract = json.loads(json.dumps(measurement_contract))
        qualification_contract["constant_set"]["qualification_thresholds"] = {
            metric: 1
            for metric in measurement_contract["measurement_protocol"]["required_metrics"]
        }
        retained = {
            "schema_version": 1,
            "status": "pass",
            "tier": "installed_runtime",
            "source": manifest["source"],
            "package": {
                "archive_sha256": "b" * 64,
                "executable_sha256": manifest["binary"]["sha256"],
                "asset_target": manifest["asset_target"],
                "release_version": manifest["release_version"],
                "model_sha256": manifest["model"]["sha256"],
                "matrix_cell_id": "installed_macos_arm64_cpu",
                "accelerator_claim": "none",
                "backend": "cpu",
                "policy": "cpu_explicit",
                "cache_state": "reused",
                "residency_state": "resident",
                "protocol_sha256": protocol_sha256,
                "constant_set_sha256": constant_set_sha256,
                "measurement_protocol_sha256": measurement_protocol_sha256,
            },
            "host": {
                "fingerprint": "f" * 64,
                "platform": "macos",
                "target": manifest["asset_target"],
                "matrix_cell_id": "installed_macos_arm64_cpu",
                "host_class": "post_publish_macos_arm64",
                "accelerator_claim": "none",
                "backend": "cpu",
                "policy": "cpu_explicit",
                "cache_state": "reused",
                "residency_state": "resident",
                "unplanned_suspend": False,
            },
            "installed_plugin": {
                "schema_version": 1,
                "installation_source": "marketplace",
                "marketplace_repository": "TheGreenCedar/AgentPluginMarketplace",
                "marketplace_commit": "d" * 40,
                "plugin_id": "codestory",
                "plugin_version": "0.0.0",
                "plugin_source_commit": manifest["source"]["commit"],
                "plugin_package_sha256": "e" * 64,
            },
            "managed_runtime": {
                "cli_source": "managed",
                "plugin_version": "0.0.0",
                "managed_binary_sha256": manifest["binary"]["sha256"],
                "archive_sha256": "b" * 64,
                "build_source": "github_release",
                "repo_ref": "v0.0.0",
                "provisioned_at": "self-test",
            },
            "same_account": {
                "account_id": "uid:501",
                "relation": "same_os_account",
                "cross_login_or_terminal_sessions_proven": False,
                "plugin_hosts": [
                    {
                        "pid": 201,
                        "process_start_id": "boot-1:201",
                        "repository_id": "repo:a",
                    },
                    {
                        "pid": 202,
                        "process_start_id": "boot-1:202",
                        "repository_id": "repo:b",
                    },
                ],
            },
            "shared_identity": shared,
            "timing": {
                "clock_domain": "awake_monotonic",
                "cross_process_timestamp_subtraction": False,
                "unplanned_suspend": False,
                "constants_frozen_before_run": True,
                "constant_set_sha256": constant_set_sha256,
            },
            "scenarios": {
                scenario_id: {
                    "status": "pass",
                    "assertions": {
                        assertion: True
                        for assertion in contract["required"]
                    },
                    "artifacts": [
                        {
                            "name": f"{scenario_id}.json",
                            "sha256": "c" * 64,
                        }
                    ],
                }
                for scenario_id, contract in scenario_contracts.items()
            },
            "lower_tier_nonclaims": {
                claim: {
                    "claimed": False,
                    "reason": "self-test lower-tier boundary",
                }
                for claim in LOWER_TIER_NONCLAIMS
            },
            "metrics": {
                metric: {
                    "status": "pass",
                    "unit": measurement_contract["measurement_protocol"]["metric_contracts"][metric]["unit"],
                    "value": 1,
                    "threshold": 1,
                    "comparison": measurement_contract["measurement_protocol"]["metric_contracts"][metric]["comparison"],
                }
                for metric in measurement_contract["measurement_protocol"]["required_metrics"]
            },
        }
        retained["scenarios"]["server_crash"]["artifacts"].append(
            publication_external["artifact"]
        )
        retained["scenarios"]["worker_stall"]["artifacts"].append(
            publication_external["artifact"]
        )
        retained["scenarios"]["server_crash"]["artifacts"].append(
            consistency_external["artifact"]
        )
        retained["metrics"]["retrieval_quality"]["raw_evidence"] = quality_external
        for metric, result in retained["metrics"].items():
            if metric != "retrieval_quality":
                result["raw_evidence"] = {
                    "name": (
                        "total-codestory-process-memory.raw.json"
                        if metric == "total_codestory_process_memory"
                        else "measurements.raw.json"
                    ),
                    "sha256": "d" * 64,
                }
        verify_retained_qualification(
            retained,
            manifest=manifest,
            archive_sha256="b" * 64,
            shared_identity=shared,
            measurement_contract=qualification_contract,
            required_tier="installed_runtime",
            required_matrix_cell_id="installed_macos_arm64_cpu",
            expected_policy="cpu_explicit",
            expected_backend="cpu",
            expected_accelerator_claim="none",
            installed_plugin=retained["installed_plugin"],
            managed_runtime=retained["managed_runtime"],
        )
        missing_scenario = json.loads(json.dumps(retained))
        missing_scenario["scenarios"].pop("frozen_owner")
        try:
            verify_retained_qualification(
                missing_scenario,
                manifest=manifest,
                archive_sha256="b" * 64,
                shared_identity=shared,
                measurement_contract=qualification_contract,
                required_tier="installed_runtime",
                required_matrix_cell_id="installed_macos_arm64_cpu",
                expected_policy="cpu_explicit",
                expected_backend="cpu",
                expected_accelerator_claim="none",
                installed_plugin=retained["installed_plugin"],
                managed_runtime=retained["managed_runtime"],
            )
        except ProofFailure:
            pass
        else:
            raise ProofFailure("incomplete installed scenario evidence was accepted")
        wrong_tier = json.loads(json.dumps(retained))
        wrong_tier["tier"] = "protected_hardware"
        try:
            verify_retained_qualification(
                wrong_tier,
                manifest=manifest,
                archive_sha256="b" * 64,
                shared_identity=shared,
                measurement_contract=qualification_contract,
                required_tier="installed_runtime",
                required_matrix_cell_id="installed_macos_arm64_cpu",
                expected_policy="cpu_explicit",
                expected_backend="cpu",
                expected_accelerator_claim="none",
                installed_plugin=retained["installed_plugin"],
                managed_runtime=retained["managed_runtime"],
            )
        except ProofFailure:
            pass
        else:
            raise ProofFailure("different-tier retained qualification was accepted")
        stale_shared = json.loads(json.dumps(retained))
        stale_shared["shared_identity"]["server_instance_id"] = "stale-server"
        try:
            verify_retained_qualification(
                stale_shared,
                manifest=manifest,
                archive_sha256="b" * 64,
                shared_identity=shared,
                measurement_contract=qualification_contract,
                required_tier="installed_runtime",
                required_matrix_cell_id="installed_macos_arm64_cpu",
                expected_policy="cpu_explicit",
                expected_backend="cpu",
                expected_accelerator_claim="none",
                installed_plugin=retained["installed_plugin"],
                managed_runtime=retained["managed_runtime"],
            )
        except ProofFailure:
            pass
        else:
            raise ProofFailure("stale retained shared server identity was accepted")
        wrong_cell = json.loads(json.dumps(retained))
        wrong_cell["package"]["matrix_cell_id"] = "protected_macos_arm64_metal"
        try:
            verify_retained_qualification(
                wrong_cell,
                manifest=manifest,
                archive_sha256="b" * 64,
                shared_identity=shared,
                measurement_contract=qualification_contract,
                required_tier="installed_runtime",
                required_matrix_cell_id="installed_macos_arm64_cpu",
                expected_policy="cpu_explicit",
                expected_backend="cpu",
                expected_accelerator_claim="none",
                installed_plugin=retained["installed_plugin"],
                managed_runtime=retained["managed_runtime"],
            )
        except ProofFailure:
            pass
        else:
            raise ProofFailure("wrong qualification matrix cell was accepted")
        invalid = {**valid, "embedding_adapter": "llvmpipe"}
        try:
            engine_identity(invalid, "accelerated", "Metal")
        except ProofFailure:
            pass
        else:
            raise ProofFailure("software adapter was accepted")
        inferred = {**valid, "embedding_execution_observation_source": "inferred_from_request"}
        try:
            engine_identity(inferred, "accelerated", "Metal")
        except ProofFailure:
            pass
        else:
            raise ProofFailure("inferred accelerator execution was accepted")

        hostile_root = root / "hostile-manifest"
        hostile_root.mkdir()
        hostile_cli = hostile_root / "codestory-cli"
        hostile_cli.write_bytes(binary_payload)
        hostile_manifest = json.loads(json.dumps(valid_manifest))
        hostile_manifest["binary"]["sha256"] = "0" * 64
        write_json(hostile_root / NATIVE_MANIFEST_FILE, hostile_manifest)
        try:
            load_native_manifest(hostile_root, hostile_cli, "0.0.0")
        except ProofFailure:
            pass
        else:
            raise ProofFailure("binary/manifest digest mismatch was accepted")

        wrong_target = json.loads(json.dumps(valid_manifest))
        wrong_target["asset_target"] = "macos-x64"
        write_json(hostile_root / NATIVE_MANIFEST_FILE, wrong_target)
        try:
            load_native_manifest(hostile_root, hostile_cli, "0.0.0")
        except ProofFailure:
            pass
        else:
            raise ProofFailure("asset target/binary architecture mismatch was accepted")

        stale_contract = json.loads(json.dumps(valid_manifest))
        stale_contract["embedding"]["query_prefix"] = "changed query: "
        write_json(hostile_root / NATIVE_MANIFEST_FILE, stale_contract)
        try:
            load_native_manifest(hostile_root, hostile_cli, "0.0.0")
        except ProofFailure:
            pass
        else:
            raise ProofFailure("stale binary embedding contract was accepted")

        marker_mismatch = json.loads(json.dumps(valid_manifest))
        marker_mismatch["engine"]["build_identity"] = build_identity.replace(
            "|end", "|note=fabricated|end"
        )
        write_json(hostile_root / NATIVE_MANIFEST_FILE, marker_mismatch)
        try:
            load_native_manifest(hostile_root, hostile_cli, "0.0.0")
        except ProofFailure:
            pass
        else:
            raise ProofFailure("binary/manifest native marker mismatch was accepted")
