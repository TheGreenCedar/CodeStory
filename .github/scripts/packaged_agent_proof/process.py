"""Process for packaged CodeStory proof."""

from .foundation import *
from .contracts import (
    ProofFailure,
    canonical_sha256,
    load_measurement_protocol,
    require,
    require_exact_keys,
    require_nonempty_string,
    require_nonnegative_int,
    require_positive_int,
    require_sha256,
    selected_qualification_matrix_cell,
    sha256,
)

def run(command: list[str], *, env: dict[str, str], cwd: Path, timeout: int) -> dict:
    started = time.perf_counter()
    completed = subprocess.run(command, cwd=cwd, env=env, text=True, capture_output=True, timeout=timeout)
    result = {
        "command": command,
        "exit_code": completed.returncode,
        "wall_ms": round((time.perf_counter() - started) * 1000, 3),
        "stdout": completed.stdout,
        "stderr": completed.stderr,
    }
    if completed.returncode != 0:
        stdout_tail = completed.stdout[-2000:].strip()
        stderr_tail = completed.stderr[-2000:].strip()
        details = "\n".join(
            part
            for part in (
                f"stdout:\n{stdout_tail}" if stdout_tail else "",
                f"stderr:\n{stderr_tail}" if stderr_tail else "",
            )
            if part
        )
        detail_suffix = f"\n{details}" if details else ""
        raise ProofFailure(
            f"command failed ({completed.returncode}): {' '.join(command)}"
            f"{detail_suffix}"
        )
    return result


def json_command(command: list[str], *, env: dict[str, str], cwd: Path, timeout: int) -> tuple[dict, dict]:
    result = run(command, env=env, cwd=cwd, timeout=timeout)
    try:
        payload = json.loads(result["stdout"])
    except json.JSONDecodeError as exc:
        raise ProofFailure(f"command did not emit JSON: {' '.join(command)}: {exc}") from exc
    require(isinstance(payload, dict), f"command emitted non-object JSON: {' '.join(command)}")
    return result, payload


def find_value(value: object, key: str) -> object | None:
    if isinstance(value, dict):
        if key in value:
            return value[key]
        for child in value.values():
            found = find_value(child, key)
            if found is not None:
                return found
    elif isinstance(value, list):
        for child in value:
            found = find_value(child, key)
            if found is not None:
                return found
    return None


def engine_identity(
    status: dict,
    expected_policy: str | None,
    expected_backend: str | None,
    *,
    expected_load_count: int = 1,
    expected_load_generation: int = 1,
    expected_residency: str = "resident",
    expected_load_error: bool = False,
) -> dict:
    fields = {
        key: find_value(status, key)
        for key in (
            "embedding_model_sha256",
            "embedding_ggml_build_identity",
            "embedding_backend",
            "embedding_adapter",
            "embedding_policy",
            "embedding_engine_instance_id",
            "embedding_engine_residency",
            "embedding_engine_load_generation",
            "embedding_engine_load_error",
            "embedding_model_load_count",
            "embedding_smoke_ms",
            "embedding_initialization_ms",
            "embedding_materialized_path",
            "embedding_materialized_reused",
            "embedding_accelerator_execution_verified",
            "embedding_execution_devices",
            "embedding_execution_backends",
            "embedding_execution_observation_source",
            "embedding_encode_count",
            "embedding_execution_node_count",
            "embedding_resident_accelerator_tensor_count",
            "embedding_resident_accelerator_tensor_bytes",
            "embedding_model_layer_count",
            "embedding_offloaded_layer_count",
        )
    }
    digest = str(fields["embedding_model_sha256"] or "")
    require(len(digest) == 64 and all(char in "0123456789abcdefABCDEF" for char in digest), "status lacks an exact model digest")
    require(bool(fields["embedding_ggml_build_identity"]), "status lacks the linked ggml build identity")
    require(bool(fields["embedding_backend"]), "status lacks the selected embedding backend")
    adapter = str(fields["embedding_adapter"] or "")
    require(adapter, "status lacks the physical adapter identity")
    require(not any(token in adapter.lower() for token in SOFTWARE_ADAPTERS), f"software adapter is not allowed: {adapter}")
    require(fields["embedding_policy"] in {"accelerated", "cpu_explicit"}, "status lacks an explicit embedding policy")
    require(bool(fields["embedding_engine_instance_id"]), "status lacks the process engine identity")
    require(fields["embedding_engine_residency"] == expected_residency, f"engine residency is {fields['embedding_engine_residency']!r}, expected {expected_residency!r}")
    require(fields["embedding_model_load_count"] == expected_load_count, f"engine load count is {fields['embedding_model_load_count']!r}, expected {expected_load_count}")
    require(fields["embedding_engine_load_generation"] == expected_load_generation, f"engine load generation is {fields['embedding_engine_load_generation']!r}, expected {expected_load_generation}")
    if expected_load_error:
        require(bool(fields["embedding_engine_load_error"]), "failed reload did not retain its load error")
    else:
        require(fields["embedding_engine_load_error"] is None, f"engine retained an unexpected load error: {fields['embedding_engine_load_error']}")
    require(isinstance(fields["embedding_smoke_ms"], (int, float)) and fields["embedding_smoke_ms"] >= 0, "status lacks the timed live embedding smoke")
    require(isinstance(fields["embedding_initialization_ms"], (int, float)) and fields["embedding_initialization_ms"] >= 0, "status lacks initialization timing")
    if expected_policy:
        require(fields["embedding_policy"] == expected_policy, f"embedding policy is {fields['embedding_policy']!r}, expected {expected_policy!r}")
    if expected_backend:
        observed = str(fields["embedding_backend"] or "").lower()
        expected = expected_backend.lower()
        matches = expected in observed or (expected == "metal" and observed == "mtl")
        require(matches, f"embedding backend is {fields['embedding_backend']!r}, expected {expected_backend!r}")
    if fields["embedding_policy"] == "accelerated":
        require(fields["embedding_accelerator_execution_verified"] is True, "accelerated policy lacks live accelerator execution proof")
        require(
            fields["embedding_execution_observation_source"] == "ggml_eval_callback",
            "accelerator execution source is unknown or inferred",
        )
        require(
            isinstance(fields["embedding_execution_devices"], list)
            and bool(fields["embedding_execution_devices"]),
            "status lacks an observed execution device",
        )
        require(
            isinstance(fields["embedding_execution_backends"], list)
            and bool(fields["embedding_execution_backends"]),
            "status lacks an observed execution backend",
        )
        require(
            isinstance(fields["embedding_encode_count"], int)
            and fields["embedding_encode_count"] > 0,
            "status lacks an advancing successful encode counter",
        )
        require(
            isinstance(fields["embedding_execution_node_count"], int)
            and fields["embedding_execution_node_count"] > 0,
            "status lacks backend-observed execution nodes",
        )
        require(
            isinstance(fields["embedding_resident_accelerator_tensor_count"], int)
            and fields["embedding_resident_accelerator_tensor_count"] > 0,
            "status lacks backend-observed resident accelerator tensors",
        )
        require(
            isinstance(fields["embedding_resident_accelerator_tensor_bytes"], int)
            and fields["embedding_resident_accelerator_tensor_bytes"] > 0,
            "status lacks backend-observed resident accelerator tensor bytes",
        )
        model_layers = fields["embedding_model_layer_count"]
        offloaded_layers = fields["embedding_offloaded_layer_count"]
        require(isinstance(model_layers, int) and model_layers > 0, "status lacks model layer count")
        require(offloaded_layers == model_layers, "not every model layer was offloaded")
    return fields


def server_snapshot(status: dict, manifest: dict, *, require_resident: bool) -> dict:
    snapshot = find_value(status, "embedding_server")
    require(isinstance(snapshot, dict), "diagnostics omitted the embedding_server snapshot")
    require(
        snapshot.get("schema_version") == SERVER_PROOF_SCHEMA_VERSION,
        "embedding_server snapshot schema is unsupported",
    )
    event_sequence = require_nonnegative_int(
        snapshot.get("event_sequence"),
        "embedding_server.event_sequence",
    )
    lifecycle = snapshot.get("lifecycle")
    require(lifecycle in SERVER_LIFECYCLES, "embedding_server lifecycle is invalid")

    clock = snapshot.get("clock")
    require(isinstance(clock, dict), "embedding_server snapshot omitted clock identity")
    require(clock.get("domain") == "awake_monotonic", "embedding_server clock is not awake-monotonic")
    require_nonempty_string(clock.get("api"), "embedding_server.clock.api")
    require_nonempty_string(clock.get("boot_id"), "embedding_server.clock.boot_id")
    require_positive_int(clock.get("resolution_ns"), "embedding_server.clock.resolution_ns")

    protocol = snapshot.get("protocol")
    require(isinstance(protocol, dict), "embedding_server snapshot omitted protocol identity")
    require(protocol.get("bootstrap_version") == 1, "embedding_server bootstrap version is unsupported")
    require(protocol.get("schema_version") == 1, "embedding_server protocol version is unsupported")
    for field in ("protocol_sha256", "constant_set_sha256", "measurement_protocol_sha256"):
        require_sha256(protocol.get(field), f"embedding_server.protocol.{field}")

    server_proof = manifest.get("server_proof")
    require(isinstance(server_proof, dict), "package manifest omitted server_proof")
    for field in (
        "bootstrap_version",
        "protocol_schema_version",
        "protocol_sha256",
        "constant_set_sha256",
        "measurement_protocol_sha256",
    ):
        runtime_field = "schema_version" if field == "protocol_schema_version" else field
        require(
            protocol.get(runtime_field) == server_proof.get(field),
            f"runtime embedding server {runtime_field} does not match the package manifest",
        )

    authority = snapshot.get("authority")
    require(isinstance(authority, dict), "embedding_server snapshot omitted authority identity")
    for field in ("endpoint_namespace_id", "lifetime_authority_id", "listener_id"):
        require_nonempty_string(authority.get(field), f"embedding_server.authority.{field}")
    require(authority.get("peer_verified") is True, "embedding_server peer identity is not verified")

    process = snapshot.get("process")
    require(isinstance(process, dict), "embedding_server snapshot omitted process identity")
    for field in ("server_instance_id", "process_start_id", "executable_version"):
        require_nonempty_string(process.get(field), f"embedding_server.process.{field}")
    require_positive_int(process.get("pid"), "embedding_server.process.pid")
    require_sha256(process.get("executable_sha256"), "embedding_server.process.executable_sha256")
    require(
        process["executable_sha256"] == manifest["binary"]["sha256"],
        "embedding server process executable does not match the package manifest",
    )
    require(
        process["executable_version"] == manifest["release_version"],
        "embedding server process version does not match the package manifest",
    )

    scheduler = snapshot.get("scheduler")
    require(isinstance(scheduler, dict), "embedding_server snapshot omitted scheduler state")
    require(
        scheduler.get("query_capacity") == server_proof.get("query_capacity") == 64,
        "embedding_server query capacity is not the manifest-bound accepted value",
    )
    require(
        scheduler.get("bulk_capacity") == server_proof.get("bulk_capacity") == 64,
        "embedding_server bulk capacity is not the manifest-bound accepted value",
    )
    for field in (
        "query_depth",
        "bulk_depth",
        "connection_count",
        "active_request_count",
        "lease_count",
    ):
        require_nonnegative_int(scheduler.get(field), f"embedding_server.scheduler.{field}")
    require(scheduler["query_depth"] <= 64, "embedding_server query depth exceeds capacity")
    require(scheduler["bulk_depth"] <= 64, "embedding_server bulk depth exceeds capacity")
    active_request = scheduler.get("active_request")
    if active_request is not None:
        require(isinstance(active_request, dict), "embedding_server active request is malformed")
        for field in ("request_id", "scope_id", "class", "phase"):
            require_nonempty_string(
                active_request.get(field),
                f"embedding_server.scheduler.active_request.{field}",
            )
        require(active_request["class"] in {"query", "bulk"}, "active request class is invalid")
        require_nonnegative_int(
            active_request.get("elapsed_ms"),
            "embedding_server.scheduler.active_request.elapsed_ms",
        )

    engine = snapshot.get("engine")
    if require_resident:
        require(isinstance(engine, dict), "resident embedding_server snapshot omitted engine identity")
    if engine is not None:
        require(isinstance(engine, dict), "embedding_server engine identity is malformed")
        for field in ("engine_owner_id", "native_worker_id"):
            require_nonempty_string(engine.get(field), f"embedding_server.engine.{field}")
        require_positive_int(engine.get("load_generation"), "embedding_server.engine.load_generation")
        require_positive_int(engine.get("model_load_count"), "embedding_server.engine.model_load_count")
        require_nonnegative_int(
            engine.get("successful_encode_count"),
            "embedding_server.engine.successful_encode_count",
        )

    failure = snapshot.get("failure")
    if failure is not None:
        require(isinstance(failure, dict), "embedding_server failure state is malformed")
        require_nonempty_string(failure.get("code"), "embedding_server.failure.code")
        require(
            failure.get("retry_class") in RETRY_CLASSES,
            "embedding_server failure retry class is invalid",
        )
        require_nonnegative_int(
            failure.get("retry_after_ms"),
            "embedding_server.failure.retry_after_ms",
        )
        require_nonempty_string(
            failure.get("retry_condition"),
            "embedding_server.failure.retry_condition",
        )

    private_tokens = (
        str(snapshot).lower()
        if not isinstance(snapshot, (str, bytes))
        else str(snapshot).lower()
    )
    for forbidden in ("project_path", "project_root", "repository_path", "request_text"):
        require(forbidden not in private_tokens, f"embedding_server diagnostics leaked {forbidden}")
    return {
        "schema_version": snapshot["schema_version"],
        "event_sequence": event_sequence,
        "lifecycle": lifecycle,
        "clock": clock,
        "protocol": protocol,
        "authority": authority,
        "process": process,
        "scheduler": scheduler,
        "engine": engine,
        "failure": failure,
    }


def shared_server_identity(first: dict, second: dict) -> dict:
    for group, fields in (
        ("authority", ("endpoint_namespace_id", "lifetime_authority_id", "listener_id")),
        ("process", ("server_instance_id", "pid", "process_start_id", "executable_sha256")),
        ("engine", ("engine_owner_id", "native_worker_id", "load_generation", "model_load_count")),
    ):
        left = first.get(group)
        right = second.get(group)
        require(isinstance(left, dict) and isinstance(right, dict), f"shared proof omitted {group}")
        for field in fields:
            require(
                left.get(field) == right.get(field),
                f"independent plugin hosts observed different {group}.{field}",
            )
    require(
        first["engine"]["model_load_count"] == 1,
        "cold two-host race produced more than one model load",
    )
    return {
        "endpoint_namespace_id": first["authority"]["endpoint_namespace_id"],
        "lifetime_authority_id": first["authority"]["lifetime_authority_id"],
        "listener_id": first["authority"]["listener_id"],
        "server_instance_id": first["process"]["server_instance_id"],
        "server_process_start_id": first["process"]["process_start_id"],
        "engine_owner_id": first["engine"]["engine_owner_id"],
        "native_worker_id": first["engine"]["native_worker_id"],
        "load_generation": first["engine"]["load_generation"],
        "model_load_count": first["engine"]["model_load_count"],
    }


def parse_byte_quantity(value: str) -> int:
    match = re.fullmatch(r"([0-9]+(?:\.[0-9]+)?)([KMG])?", value.strip())
    require(match is not None, f"invalid memory quantity: {value!r}")
    scale = {None: 1, "K": 1024, "M": 1024**2, "G": 1024**3}[match.group(2)]
    return round(float(match.group(1)) * scale)


def process_resident_memory(pid: int) -> tuple[int, str]:
    if os.name == "nt":
        command = [
            "powershell",
            "-NoProfile",
            "-Command",
            f"(Get-Process -Id {pid} -ErrorAction Stop).WorkingSet64",
        ]
        scale = 1
        metric = "windows_working_set"
    elif sys.platform == "darwin":
        completed = subprocess.run(
            ["vmmap", "-summary", str(pid)],
            text=True,
            capture_output=True,
            timeout=20,
        )
        require(completed.returncode == 0, f"could not read physical footprint for process {pid}: {completed.stderr.strip()}")
        match = re.search(r"^Physical footprint:\s+([^\s]+)", completed.stdout, re.MULTILINE)
        require(match is not None, f"vmmap omitted the physical footprint for process {pid}")
        return parse_byte_quantity(match.group(1)), "macos_physical_footprint"
    else:
        command = ["ps", "-o", "rss=", "-p", str(pid)]
        scale = 1024
        metric = "rss"
    completed = subprocess.run(command, text=True, capture_output=True, timeout=10)
    require(completed.returncode == 0, f"could not read RSS for process {pid}: {completed.stderr.strip()}")
    try:
        return int(completed.stdout.strip()) * scale, metric
    except ValueError as exc:
        raise ProofFailure(f"invalid RSS for process {pid}: {completed.stdout!r}") from exc


def suspend_clock_pair(target_os: str) -> tuple[int, int, str, str]:
    awake_ns = time.monotonic_ns()
    if target_os == "linux":
        require(
            hasattr(time, "CLOCK_BOOTTIME"),
            "Linux qualification host lacks CLOCK_BOOTTIME",
        )
        inclusive_ns = time.clock_gettime_ns(time.CLOCK_BOOTTIME)
        return awake_ns, inclusive_ns, "CLOCK_MONOTONIC", "CLOCK_BOOTTIME"
    if target_os == "macos":
        class MachTimebaseInfo(ctypes.Structure):
            _fields_ = [("numer", ctypes.c_uint32), ("denom", ctypes.c_uint32)]

        system = ctypes.CDLL("/usr/lib/libSystem.B.dylib")
        system.mach_continuous_time.restype = ctypes.c_uint64
        system.mach_timebase_info.argtypes = [ctypes.POINTER(MachTimebaseInfo)]
        info = MachTimebaseInfo()
        require(
            system.mach_timebase_info(ctypes.byref(info)) == 0 and info.denom > 0,
            "macOS qualification host could not read mach timebase",
        )
        inclusive_ticks = system.mach_continuous_time()
        inclusive_ns = inclusive_ticks * info.numer // info.denom
        return awake_ns, inclusive_ns, "mach_absolute_time", "mach_continuous_time"
    require(target_os == "windows", f"unsupported qualification clock target {target_os}")
    kernel = ctypes.windll.kernel32
    unbiased = ctypes.c_ulonglong()
    inclusive = ctypes.c_ulonglong()
    require(
        bool(kernel.QueryUnbiasedInterruptTimePrecise(ctypes.byref(unbiased))),
        "Windows qualification host could not read unbiased interrupt time",
    )
    kernel.QueryInterruptTimePrecise(ctypes.byref(inclusive))
    return (
        int(unbiased.value) * 100,
        int(inclusive.value) * 100,
        "QueryUnbiasedInterruptTimePrecise",
        "QueryInterruptTimePrecise",
    )


def plugin_client_process(
    status: dict,
    manifest: dict,
    label: str,
    *,
    target_os: str,
) -> dict:
    plugin_runtime = status.get("plugin_runtime")
    require(isinstance(plugin_runtime, dict), f"{label} omitted plugin_runtime")
    process = plugin_runtime.get("client_process")
    require(isinstance(process, dict), f"{label} omitted client_process")
    require_exact_keys(
        process,
        {"pid", "process_start_id", "executable_sha256"},
        f"{label} client_process",
    )
    pid = require_positive_int(process["pid"], f"{label} client_process.pid")
    start_id = require_nonempty_string(
        process["process_start_id"],
        f"{label} client_process.process_start_id",
    )
    return verified_live_executable(
        pid=pid,
        process_start_id=start_id,
        reported_sha256=process["executable_sha256"],
        expected_sha256=manifest["binary"]["sha256"],
        target_os=target_os,
        label=f"{label} client process",
    )


def capture_five_process_memory(
    *,
    args: argparse.Namespace,
    node_path: Path,
    host_a: McpProcess,
    host_a_start: str,
    host_b: McpProcess,
    host_b_start: str,
    status_a: dict,
    status_b: dict,
    snapshot: dict,
    manifest: dict,
    expected_backend: str,
) -> dict:
    protocol, _ = load_measurement_protocol(args.measurement_protocol)
    matrix_cell_id = require_nonempty_string(
        args.qualification_matrix_cell,
        "memory qualification requires --qualification-matrix-cell",
    )
    matrix_cell = selected_qualification_matrix_cell(
        protocol,
        cell_id=matrix_cell_id,
        target=manifest["asset_target"],
        proof_tier=args.proof_tier,
        expected_policy=args.engine_policy,
        expected_backend=expected_backend,
    )
    target_os = TARGET_CONTRACTS[manifest["asset_target"]]["target_os"]
    client_a = plugin_client_process(
        status_a,
        manifest,
        "first plugin host",
        target_os=target_os,
    )
    client_b = plugin_client_process(
        status_b,
        manifest,
        "second plugin host",
        target_os=target_os,
    )
    require(
        (client_a["pid"], client_a["process_start_id"])
        != (client_b["pid"], client_b["process_start_id"]),
        "plugin hosts reported the same CLI client process",
    )
    server = snapshot["process"]
    server_live = verified_live_executable(
        pid=require_positive_int(server.get("pid"), "embedding server pid"),
        process_start_id=require_nonempty_string(
            server.get("process_start_id"),
            "embedding server process_start_id",
        ),
        reported_sha256=server.get("executable_sha256"),
        expected_sha256=manifest["binary"]["sha256"],
        target_os=target_os,
        label="embedding server process",
    )
    node_digest = sha256(node_path.resolve())
    process_set = [
        {
            "role": "plugin_host_a",
            "pid": host_a.process.pid,
            "process_start_id": host_a_start,
            "executable_sha256": node_digest,
        },
        {"role": "plugin_cli_a", **client_a},
        {
            "role": "plugin_host_b",
            "pid": host_b.process.pid,
            "process_start_id": host_b_start,
            "executable_sha256": node_digest,
        },
        {"role": "plugin_cli_b", **client_b},
        {
            "role": "embedding_server",
            **server_live,
        },
    ]
    identities = {
        (process["pid"], process["process_start_id"]) for process in process_set
    }
    require(
        len(identities) == 5,
        "memory evidence did not identify five distinct live CodeStory processes",
    )
    boot_id = require_nonempty_string(
        snapshot["clock"]["boot_id"],
        "embedding server clock boot_id",
    )
    samples = []
    for repeat in range(1, 4):
        awake_started, inclusive_started, awake_api, inclusive_api = (
            suspend_clock_pair(target_os)
        )
        processes = []
        for process in process_set:
            require(
                process_start_identity(process["pid"])
                == process["process_start_id"],
                f"memory process {process['role']} changed identity before sampling",
            )
            resident_bytes, measurement_api = process_resident_memory(process["pid"])
            processes.append(
                {
                    **process,
                    "resident_bytes": resident_bytes,
                    "measurement_api": measurement_api,
                }
            )
        awake_finished, inclusive_finished, finished_awake_api, finished_inclusive_api = (
            suspend_clock_pair(target_os)
        )
        require(
            finished_awake_api == awake_api
            and finished_inclusive_api == inclusive_api,
            "memory sampling clock API changed within one sample",
        )
        for process in process_set:
            require(
                process_start_identity(process["pid"])
                == process["process_start_id"],
                f"memory process {process['role']} changed identity during sampling",
            )
        samples.append(
            {
                "sample_id": canonical_sha256(
                    {
                        "matrix_cell_id": matrix_cell_id,
                        "repeat": repeat,
                        "identities": sorted(identities),
                    }
                ),
                "repeat": repeat,
                "matrix_cell_id": matrix_cell_id,
                "workload_id": protocol["workloads"][
                    "total_codestory_process_memory"
                ]["workload_id"],
                "cache_state": matrix_cell["cache_state"],
                "residency_state": matrix_cell["residency_state"],
                "producer_process": {
                    "pid": os.getpid(),
                    "process_start_id": process_start_identity(os.getpid()),
                },
                "server_identity": {
                    "server_instance_id": snapshot["process"]["server_instance_id"],
                    "process_start_id": snapshot["process"]["process_start_id"],
                    "load_generation": snapshot["engine"]["load_generation"],
                },
                "clock": {
                    "domain": "awake_monotonic",
                    "api": awake_api,
                    "boot_id": boot_id,
                    "resolution_ns": max(1, round(time.get_clock_info("monotonic").resolution * 1e9)),
                },
                "start": {
                    "phase": "steady_state_process_set_identified",
                    "observed_ns": awake_started,
                },
                "end": {
                    "phase": "steady_state_memory_samples_collected",
                    "observed_ns": awake_finished,
                },
                "operands": {"processes": processes},
                "suspend_witness": {
                    "awake_started_ns": awake_started,
                    "awake_finished_ns": awake_finished,
                    "inclusive_clock_api": inclusive_api,
                    "inclusive_started_ns": inclusive_started,
                    "inclusive_finished_ns": inclusive_finished,
                    "boot_id_started": boot_id,
                    "boot_id_finished": boot_id,
                },
            }
        )
        if repeat < 3:
            time.sleep(0.25)
    return {
        "evidence_contract": MEMORY_EVIDENCE_CONTRACT,
        "metric": "total_codestory_process_memory",
        "unit": "bytes",
        "samples": samples,
    }


def assert_public_status(status: dict) -> None:
    require(find_value(status, "retrieval_mode") == "full", "public status does not report full retrieval")
    maintainer_only = (
        "sidecar",
        "full_repair",
        "embedding_model_sha256",
        "embedding_ggml_build_identity",
        "embedding_backend",
        "embedding_adapter",
        "embedding_policy",
        "embedding_engine_instance_id",
        "embedding_engine_residency",
        "embedding_engine_load_generation",
        "embedding_engine_load_error",
        "embedding_materialized_path",
        "embedding_detected_provider",
        "embedding_detected_gpu",
        "embedding_server",
        "server_instance_id",
        "lifetime_authority_id",
        "listener_id",
        "engine_owner_id",
        "native_worker_id",
        "constant_set_sha256",
        "measurement_protocol_sha256",
    )
    leaked = [key for key in maintainer_only if find_value(status, key) is not None]
    require(not leaked, "public status leaked maintainer-only retrieval fields: " + ", ".join(leaked))


def extract_resource(
    response: dict,
    uri: str,
    *,
    platform_name: str | None = None,
    samefile=None,
) -> dict:
    require("error" not in response, f"resource read failed: {response.get('error')}")
    contents = response.get("result", {}).get("contents", [])
    for item in contents:
        if (
            isinstance(item, dict)
            and isinstance(item.get("uri"), str)
            and resource_uri_matches(
                uri,
                item["uri"],
                platform_name=platform_name,
                samefile=samefile,
            )
        ):
            payload = json.loads(item.get("text", "{}"))
            require(isinstance(payload, dict), "resource emitted non-object JSON")
            return payload
    raise ProofFailure(f"resource response did not contain {uri}")


class McpProcess:
    def __init__(self, command: list[str], *, env: dict[str, str], cwd: Path, timeout: int):
        self.timeout = timeout
        self.process = subprocess.Popen(command, cwd=cwd, env=env, text=True, stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
        self.lines: queue.Queue[str | None] = queue.Queue()
        self.stderr: list[str] = []
        assert self.process.stdout and self.process.stderr and self.process.stdin
        threading.Thread(target=self._reader, args=(self.process.stdout, self.lines), daemon=True).start()
        threading.Thread(target=self._stderr_reader, daemon=True).start()
        self.transcript: list[dict] = []
        self.tool_attempt_counts: dict[str, int] = {}

    @staticmethod
    def _reader(stream, output: queue.Queue[str | None]) -> None:
        for line in stream:
            output.put(line)
        output.put(None)

    def _stderr_reader(self) -> None:
        assert self.process.stderr
        self.stderr.extend(self.process.stderr.readlines())

    def send(self, request: dict) -> dict:
        assert self.process.stdin
        self.process.stdin.write(json.dumps(request) + "\n")
        self.process.stdin.flush()
        deadline = time.monotonic() + self.timeout
        while True:
            remaining = deadline - time.monotonic()
            require(remaining > 0, f"MCP request timed out: {request.get('id')}")
            try:
                line = self.lines.get(timeout=remaining)
            except queue.Empty as exc:
                raise ProofFailure(f"MCP request timed out: {request.get('id')}") from exc
            require(line is not None, f"MCP process closed: {''.join(self.stderr)[-2000:]}")
            response = json.loads(line)
            self.transcript.append({"request": request, "response": response})
            if response.get("id") == request.get("id"):
                return response

    def initialize(self) -> None:
        response = self.send({
            "jsonrpc": "2.0",
            "id": "initialize",
            "method": "initialize",
            "params": {"protocolVersion": "2024-11-05", "capabilities": {}, "clientInfo": {"name": "packaged-proof", "version": "1"}},
        })
        require("error" not in response, f"MCP initialize failed: {response.get('error')}")
        assert self.process.stdin
        self.process.stdin.write(json.dumps({"jsonrpc": "2.0", "method": "notifications/initialized"}) + "\n")
        self.process.stdin.flush()

    def status(self, project: Path, request_id: str) -> dict:
        uri = project_resource_uri(STATUS_URI, project)
        return extract_resource(self.send({
            "jsonrpc": "2.0",
            "id": request_id,
            "method": "resources/read",
            "params": {"uri": uri},
        }), uri)

    def engine_diagnostics(self, project: Path, request_id: str) -> dict:
        uri = project_resource_uri(ENGINE_DIAGNOSTICS_URI, project)
        return extract_resource(self.send({
            "jsonrpc": "2.0",
            "id": request_id,
            "method": "resources/read",
            "params": {"uri": uri},
        }), uri)

    def resource(self, uri: str, request_id: str) -> dict:
        return extract_resource(self.send({
            "jsonrpc": "2.0",
            "id": request_id,
            "method": "resources/read",
            "params": {"uri": uri},
        }), uri)

    def tool(self, name: str, arguments: dict, request_id: str) -> dict:
        response = self.send({"jsonrpc": "2.0", "id": request_id, "method": "tools/call", "params": {"name": name, "arguments": arguments}})
        require("error" not in response, f"MCP {name} failed: {response.get('error')}")
        return response

    def tool_until_ready(self, name: str, arguments: dict, request_id: str) -> tuple[dict, int]:
        deadline = time.monotonic() + self.timeout
        attempt = 0
        while True:
            attempt += 1
            self.tool_attempt_counts[request_id] = attempt
            response = self.tool(name, arguments, f"{request_id}-{attempt}")
            result = response.get("result")
            require(
                isinstance(result, dict),
                f"MCP {name} attempt {attempt} returned a non-object result: {result!r}",
            )
            state = result.get("structuredContent")
            require(
                isinstance(state, dict),
                f"MCP {name} attempt {attempt} returned non-object structuredContent: {result!r}",
            )
            is_error = result.get("isError")
            if "isError" not in result or is_error is False:
                return response, attempt
            require(
                is_error is True,
                f"MCP {name} attempt {attempt} returned invalid isError={is_error!r}: {result!r}",
            )
            retry_state = (state.get("code"), state.get("state"))
            require(
                retry_state
                in (
                    ("codestory_preparing", "preparing"),
                    ("codestory_updating", "updating"),
                ),
                f"MCP {name} attempt {attempt} returned a terminal or malformed error envelope: {state!r}",
            )
            require(
                state.get("retry_tool") == name,
                f"MCP {name} attempt {attempt} returned the wrong retry tool: {state!r}",
            )
            retry_after_ms = state.get("retry_after_ms")
            require(
                isinstance(retry_after_ms, int)
                and not isinstance(retry_after_ms, bool)
                and retry_after_ms >= 0,
                f"MCP {name} attempt {attempt} returned invalid retry_after_ms: {state!r}",
            )
            remaining = deadline - time.monotonic()
            require(
                remaining > 0,
                f"MCP {name} did not become ready after attempt {attempt}: {state!r}",
            )
            delay_ms = min(retry_after_ms, max(0, int(remaining * 1000)))
            time.sleep(delay_ms / 1000)

    def search_until_ready(self, arguments: dict, request_id: str) -> tuple[dict, int]:
        response, attempts = self.tool_until_ready("search", arguments, request_id)
        result = response["result"]
        state = result["structuredContent"]
        query = arguments.get("query")
        require(
            isinstance(query, str) and state.get("query") == query,
            f"MCP search returned a mismatched query: expected {query!r}, response={state!r}",
        )
        require(
            isinstance(state.get("hits"), list),
            f"MCP search returned non-array hits: {state!r}",
        )
        retrieval = state.get("retrieval")
        require(
            isinstance(retrieval, dict) and retrieval.get("state") == "ready",
            f"MCP search did not return the ready installed retrieval projection: {state!r}",
        )
        # The installed result is deliberately compact. Full retrieval remains
        # proven separately by public status and activation diagnostics.
        return response, attempts

    def close(self) -> None:
        if self.process.stdin:
            self.process.stdin.close()
        try:
            self.process.wait(timeout=5)
        except subprocess.TimeoutExpired:
            self.process.terminate()
            try:
                self.process.wait(timeout=5)
            except subprocess.TimeoutExpired:
                self.process.kill()
                self.process.wait(timeout=5)

    def kill(self) -> None:
        if self.process.poll() is None:
            self.process.kill()
            self.process.wait(timeout=10)


def process_start_identity(pid: int) -> str:
    if os.name == "nt":
        class FileTime(ctypes.Structure):
            _fields_ = [
                ("low_date_time", ctypes.c_uint32),
                ("high_date_time", ctypes.c_uint32),
            ]

        kernel = ctypes.windll.kernel32
        kernel.OpenProcess.argtypes = [ctypes.c_uint32, ctypes.c_int, ctypes.c_uint32]
        kernel.OpenProcess.restype = ctypes.c_void_p
        kernel.GetProcessTimes.argtypes = [
            ctypes.c_void_p,
            ctypes.POINTER(FileTime),
            ctypes.POINTER(FileTime),
            ctypes.POINTER(FileTime),
            ctypes.POINTER(FileTime),
        ]
        kernel.GetProcessTimes.restype = ctypes.c_int
        kernel.GetExitCodeProcess.argtypes = [
            ctypes.c_void_p,
            ctypes.POINTER(ctypes.c_uint32),
        ]
        kernel.GetExitCodeProcess.restype = ctypes.c_int
        kernel.CloseHandle.argtypes = [ctypes.c_void_p]
        handle = kernel.OpenProcess(0x1000, 0, pid)
        require(bool(handle), f"could not open process {pid} for start identity")
        try:
            creation = FileTime()
            exit_time = FileTime()
            kernel_time = FileTime()
            user_time = FileTime()
            require(
                bool(
                    kernel.GetProcessTimes(
                        handle,
                        ctypes.byref(creation),
                        ctypes.byref(exit_time),
                        ctypes.byref(kernel_time),
                        ctypes.byref(user_time),
                    )
                ),
                f"could not read process start identity for {pid}",
            )
            exit_code = ctypes.c_uint32()
            require(
                bool(kernel.GetExitCodeProcess(handle, ctypes.byref(exit_code)))
                and exit_code.value == 259
                and exit_time.low_date_time == 0
                and exit_time.high_date_time == 0,
                f"process {pid} was not running during start-identity inspection",
            )
        finally:
            kernel.CloseHandle(handle)
        filetime_ticks = (creation.high_date_time << 32) | creation.low_date_time
        # Match codestory-retrieval's legacy DateTime-tick serialization exactly.
        creation_ticks = (filetime_ticks // 10 * 10) + 504_911_232_000_000_000
        return f"windows:{creation_ticks}"
    if sys.platform == "linux":
        stat = Path(f"/proc/{pid}/stat").read_text(encoding="utf-8")
        fields = stat.rsplit(") ", 1)
        require(len(fields) == 2, f"/proc/{pid}/stat omitted process start identity")
        process_fields = fields[1].split()
        require(len(process_fields) > 19, f"/proc/{pid}/stat omitted process start identity")
        return f"linux:{process_fields[19]}"
    if sys.platform == "darwin":
        class ProcBsdInfo(ctypes.Structure):
            _fields_ = [
                ("pbi_flags", ctypes.c_uint32),
                ("pbi_status", ctypes.c_uint32),
                ("pbi_xstatus", ctypes.c_uint32),
                ("pbi_pid", ctypes.c_uint32),
                ("pbi_ppid", ctypes.c_uint32),
                ("pbi_uid", ctypes.c_uint32),
                ("pbi_gid", ctypes.c_uint32),
                ("pbi_ruid", ctypes.c_uint32),
                ("pbi_rgid", ctypes.c_uint32),
                ("pbi_svuid", ctypes.c_uint32),
                ("pbi_svgid", ctypes.c_uint32),
                ("rfu_1", ctypes.c_uint32),
                ("pbi_comm", ctypes.c_char * 16),
                ("pbi_name", ctypes.c_char * 32),
                ("pbi_nfiles", ctypes.c_uint32),
                ("pbi_pgid", ctypes.c_uint32),
                ("pbi_pjobc", ctypes.c_uint32),
                ("e_tdev", ctypes.c_uint32),
                ("e_tpgid", ctypes.c_uint32),
                ("pbi_nice", ctypes.c_int32),
                ("pbi_start_tvsec", ctypes.c_uint64),
                ("pbi_start_tvusec", ctypes.c_uint64),
            ]

        libproc = ctypes.CDLL("/usr/lib/libproc.dylib", use_errno=True)
        libproc.proc_pidinfo.argtypes = [
            ctypes.c_int,
            ctypes.c_int,
            ctypes.c_uint64,
            ctypes.c_void_p,
            ctypes.c_int,
        ]
        libproc.proc_pidinfo.restype = ctypes.c_int
        info = ProcBsdInfo()
        expected = ctypes.sizeof(info)
        read = libproc.proc_pidinfo(pid, 3, 0, ctypes.byref(info), expected)
        require(
            read == expected and info.pbi_pid == pid,
            f"could not read complete process start identity for {pid}",
        )
        return f"macos-proc:{info.pbi_start_tvsec}:{info.pbi_start_tvusec}"
    completed = subprocess.run(
        ["ps", "-o", "lstart=", "-p", str(pid)],
        text=True,
        capture_output=True,
        timeout=20,
    )
    require(completed.returncode == 0, f"could not read process start identity for {pid}")
    return "unix:" + require_nonempty_string(
        completed.stdout.strip(), "process start identity"
    )


class ExactProcessExitWaiter:
    def __init__(self, pid: int, expected_start_id: str, target_os: str):
        self.pid = pid
        self.expected_start_id = require_native_process_start_identity(
            expected_start_id,
            target_os,
            f"process {pid} expected exit-wait identity",
        )
        self.target_os = target_os
        self.handle = None
        host_os = (
            "windows"
            if os.name == "nt"
            else ("macos" if sys.platform == "darwin" else "linux")
        )
        require(
            target_os == host_os,
            f"cannot wait for a {target_os} process on a {host_os} host",
        )
        if target_os == "windows":
            kernel = ctypes.windll.kernel32
            kernel.OpenProcess.argtypes = [ctypes.c_uint32, ctypes.c_int, ctypes.c_uint32]
            kernel.OpenProcess.restype = ctypes.c_void_p
            self.handle = kernel.OpenProcess(0x00100000 | 0x1000, 0, pid)
            require(bool(self.handle), f"could not open exact process {pid} for exit wait")
        try:
            require(
                process_start_identity(pid) == self.expected_start_id,
                f"process {pid} changed identity before exit wait",
            )
        except BaseException:
            self.close()
            raise

    def wait(self, timeout_ms: int, *, require_clean_exit: bool = True) -> dict:
        require(timeout_ms > 0, "exact process exit wait requires a positive timeout")
        if self.target_os == "windows":
            kernel = ctypes.windll.kernel32
            kernel.WaitForSingleObject.argtypes = [ctypes.c_void_p, ctypes.c_uint32]
            kernel.WaitForSingleObject.restype = ctypes.c_uint32
            kernel.GetExitCodeProcess.argtypes = [
                ctypes.c_void_p,
                ctypes.POINTER(ctypes.c_uint32),
            ]
            kernel.GetExitCodeProcess.restype = ctypes.c_int
            result = kernel.WaitForSingleObject(self.handle, timeout_ms)
            require(
                result == 0,
                (
                    f"exact process {self.pid} did not exit within {timeout_ms}ms"
                    if result == 258
                    else f"exact process {self.pid} exit wait failed with result {result}"
                ),
            )
            exit_code = ctypes.c_uint32()
            require(
                bool(kernel.GetExitCodeProcess(self.handle, ctypes.byref(exit_code))),
                f"could not read exact process {self.pid} exit code",
            )
            if require_clean_exit:
                require(
                    exit_code.value == 0,
                    f"exact process {self.pid} exited abnormally with code {exit_code.value}",
                )
        else:
            exit_code = None
            deadline = time.monotonic() + (timeout_ms / 1000)
            while True:
                try:
                    current_identity = process_start_identity(self.pid)
                except (FileNotFoundError, ProcessLookupError):
                    break
                except ProofFailure:
                    try:
                        os.kill(self.pid, 0)
                    except ProcessLookupError:
                        break
                    raise
                require(
                    current_identity == self.expected_start_id,
                    f"process {self.pid} changed identity during exit wait",
                )
                require(
                    time.monotonic() < deadline,
                    f"exact process {self.pid} did not exit within {timeout_ms}ms",
                )
                time.sleep(0.01)
        return {
            "status": (
                (
                    "normal_idle_exit"
                    if exit_code.value == 0
                    else "superseded_process_exit"
                )
                if self.target_os == "windows"
                else "observed_exit"
            ),
            "pid": self.pid,
            "process_start_id": self.expected_start_id,
            "exit_code": exit_code.value if exit_code is not None else None,
            "clean_exit_required": require_clean_exit,
            "timeout_ms": timeout_ms,
        }

    def close(self) -> None:
        if self.handle is not None:
            kernel = ctypes.windll.kernel32
            kernel.CloseHandle.argtypes = [ctypes.c_void_p]
            kernel.CloseHandle(self.handle)
            self.handle = None


def add_exception_note(error: BaseException, note: str) -> None:
    add_note = getattr(error, "add_note", None)
    if callable(add_note):
        add_note(note)
        return
    notes = list(getattr(error, "__notes__", []))
    notes.append(note)
    error.__notes__ = notes
    if error.args:
        error.args = (f"{error.args[0]}\nsecondary context: {note}", *error.args[1:])
    else:
        error.args = (f"secondary context: {note}",)


class FailurePreservingTemporaryDirectory(tempfile.TemporaryDirectory):
    def __init__(
        self,
        *args,
        cleanup_retry_budget_secs: float = 0,
        cleanup_retry_interval_secs: float = 0.5,
        **kwargs,
    ):
        super().__init__(*args, **kwargs)
        self.cleanup_retry_budget_secs = cleanup_retry_budget_secs
        self.cleanup_retry_interval_secs = cleanup_retry_interval_secs

    def __exit__(self, exc_type, exc, traceback) -> bool | None:
        deadline = time.monotonic() + self.cleanup_retry_budget_secs
        try:
            while True:
                try:
                    self.cleanup()
                    return None
                except OSError:
                    if time.monotonic() >= deadline:
                        raise
                    time.sleep(
                        min(
                            self.cleanup_retry_interval_secs,
                            max(0, deadline - time.monotonic()),
                        )
                    )
        except OSError as cleanup_error:
            if exc is None:
                raise
            add_exception_note(
                exc,
                "temporary package directory cleanup also failed: "
                f"{cleanup_error}",
            )
            return False


def require_native_process_start_identity(
    identity: object,
    target_os: str,
    label: str,
) -> str:
    value = require_nonempty_string(identity, label)
    patterns = {
        "linux": r"linux:[0-9]+",
        "macos": r"macos-proc:[0-9]+:[0-9]+",
        "windows": r"windows:[0-9]+",
    }
    require(target_os in patterns, f"{label} used unsupported target OS {target_os}")
    require(
        re.fullmatch(patterns[target_os], value) is not None,
        f"{label} did not use the canonical {target_os} process identity format",
    )
    return value


def live_process_executable_sha256(
    pid: int,
    expected_start_id: str,
    target_os: str,
) -> str:
    expected_start_id = require_native_process_start_identity(
        expected_start_id,
        target_os,
        f"process {pid} expected start identity",
    )
    require(
        process_start_identity(pid) == expected_start_id,
        f"process {pid} changed identity before executable-image inspection",
    )
    if target_os == "linux":
        descriptor = os.open(f"/proc/{pid}/exe", os.O_RDONLY)
    elif target_os == "macos":
        libproc = ctypes.CDLL("/usr/lib/libproc.dylib")
        libproc.proc_pidpath.argtypes = [
            ctypes.c_int,
            ctypes.c_void_p,
            ctypes.c_uint32,
        ]
        libproc.proc_pidpath.restype = ctypes.c_int
        buffer = ctypes.create_string_buffer(4096)
        length = libproc.proc_pidpath(pid, buffer, len(buffer))
        require(length > 0, f"proc_pidpath could not inspect process {pid}")
        executable_path = os.fsdecode(buffer.raw[:length].split(b"\0", 1)[0])
        descriptor = os.open(executable_path, os.O_RDONLY)
    else:
        require(target_os == "windows", f"unsupported executable-image target {target_os}")
        kernel = ctypes.windll.kernel32
        kernel.OpenProcess.argtypes = [ctypes.c_uint32, ctypes.c_int, ctypes.c_uint32]
        kernel.OpenProcess.restype = ctypes.c_void_p
        kernel.QueryFullProcessImageNameW.argtypes = [
            ctypes.c_void_p,
            ctypes.c_uint32,
            ctypes.c_wchar_p,
            ctypes.POINTER(ctypes.c_uint32),
        ]
        kernel.QueryFullProcessImageNameW.restype = ctypes.c_int
        kernel.CloseHandle.argtypes = [ctypes.c_void_p]
        handle = kernel.OpenProcess(0x1000, 0, pid)
        require(bool(handle), f"OpenProcess could not inspect process {pid}")
        try:
            buffer = ctypes.create_unicode_buffer(32768)
            length = ctypes.c_uint32(len(buffer))
            require(
                bool(
                    kernel.QueryFullProcessImageNameW(
                        handle,
                        0,
                        buffer,
                        ctypes.byref(length),
                    )
                ),
                f"QueryFullProcessImageNameW could not inspect process {pid}",
            )
            executable_path = buffer.value[: length.value]
        finally:
            kernel.CloseHandle(handle)
        descriptor = os.open(executable_path, os.O_RDONLY | getattr(os, "O_BINARY", 0))
    digest = hashlib.sha256()
    try:
        while True:
            chunk = os.read(descriptor, 1024 * 1024)
            if not chunk:
                break
            digest.update(chunk)
    finally:
        os.close(descriptor)
    require(
        process_start_identity(pid) == expected_start_id,
        f"process {pid} changed identity during executable-image inspection",
    )
    return digest.hexdigest()


def verified_live_executable(
    *,
    pid: int,
    process_start_id: str,
    reported_sha256: str,
    expected_sha256: str,
    target_os: str,
    label: str,
) -> dict:
    require_sha256(reported_sha256, f"{label} reported executable sha256")
    require_sha256(expected_sha256, f"{label} expected executable sha256")
    live_sha256 = live_process_executable_sha256(pid, process_start_id, target_os)
    require(
        live_sha256 == reported_sha256 == expected_sha256,
        f"{label} live executable image does not match its reported and packaged digest",
    )
    return {
        "pid": pid,
        "process_start_id": process_start_id,
        "executable_sha256": live_sha256,
    }


def pin_temporary_package_server(
    control: dict,
    server_process: dict,
    manifest: dict,
    target_os: str,
    label: str,
) -> dict:
    pid = require_positive_int(server_process.get("pid"), f"{label} pid")
    process_start_id = require_native_process_start_identity(
        server_process.get("process_start_id"),
        target_os,
        f"{label} process start identity",
    )
    waiters = control.setdefault("_waiters", [])
    require(isinstance(waiters, list), "temporary package cleanup waiter state is invalid")
    for entry in waiters:
        require(isinstance(entry, dict), "temporary package cleanup waiter is malformed")
        if entry.get("identity") == (pid, process_start_id):
            return entry
    verified_process = verified_live_executable(
        pid=pid,
        process_start_id=process_start_id,
        reported_sha256=server_process["executable_sha256"],
        expected_sha256=manifest["binary"]["sha256"],
        target_os=target_os,
        label=label,
    )
    entry = {
        "identity": (pid, process_start_id),
        "server_instance_id": require_nonempty_string(
            server_process.get("server_instance_id"),
            f"{label} server instance",
        ),
        "executable_sha256": verified_process["executable_sha256"],
        "waiter": ExactProcessExitWaiter(pid, process_start_id, target_os),
    }
    waiters.append(entry)
    return entry


def native_server_exit_wait_required(target_os: str, proof_tier: str) -> bool:
    return target_os == "windows" and proof_tier in {
        "calibration",
        "hosted_package",
        "protected_hardware",
        "installed_runtime",
    }


def native_server_exit_wait_budget(manifest: dict) -> dict:
    product_idle_timeout_ms = require_positive_int(
        manifest["server_proof"].get("idle_timeout_ms"),
        "native server product idle timeout",
    )
    return {
        "product_idle_timeout_ms": product_idle_timeout_ms,
        "native_teardown_grace_ms": NATIVE_SERVER_TEARDOWN_GRACE_MS,
        "timeout_ms": product_idle_timeout_ms + NATIVE_SERVER_TEARDOWN_GRACE_MS,
    }


def remaining_native_server_exit_wait_ms(
    deadline: float,
    timeout_ms: int,
    *,
    now: float | None = None,
) -> int:
    current = time.monotonic() if now is None else now
    remaining_ms = math.ceil((deadline - current) * 1000)
    require(
        remaining_ms > 0,
        f"native server cleanup exceeded shared {timeout_ms}ms exit-wait bound",
    )
    return min(timeout_ms, remaining_ms)


def retained_final_native_server_exit_evidence(
    evidence: dict,
    final_entry: dict,
    wait_budget: dict,
    *,
    authenticated_process_count: int,
    superseded_process_count: int,
) -> dict:
    require(
        evidence.get("status") == "normal_idle_exit"
        and evidence.get("exit_code") == 0
        and evidence.get("clean_exit_required") is True,
        "final authenticated server did not prove its clean idle exit",
    )
    require(
        (evidence.get("pid"), evidence.get("process_start_id"))
        == final_entry.get("identity"),
        "final server cleanup evidence changed exact process identity",
    )
    executable_sha256 = require_sha256(
        final_entry.get("executable_sha256"),
        "final server cleanup executable sha256",
    )
    process_wait_timeout_ms = require_positive_int(
        evidence.get("timeout_ms"),
        "final server cleanup process wait timeout",
    )
    require(
        process_wait_timeout_ms <= wait_budget.get("timeout_ms"),
        "final server cleanup process wait exceeded its shared bound",
    )
    return {
        **evidence,
        "observation": "final_temporary_directory_boundary",
        "server_instance_id": final_entry["server_instance_id"],
        "executable_sha256": executable_sha256,
        "process_wait_timeout_ms": process_wait_timeout_ms,
        **wait_budget,
        "authenticated_process_count": authenticated_process_count,
        "superseded_process_count": superseded_process_count,
    }


def wait_for_final_temporary_package_server(
    args: argparse.Namespace,
    env: dict[str, str],
    control: dict,
    manifest: dict,
    *,
    require_final_server: bool,
) -> dict | None:
    target_os = TARGET_CONTRACTS[manifest["asset_target"]]["target_os"]
    if not native_server_exit_wait_required(target_os, args.proof_tier):
        return None
    waiters = control.setdefault("_waiters", [])
    require(isinstance(waiters, list), "temporary package cleanup waiter state is invalid")
    observation_error = None
    host_close_error = None
    final_entry = None
    snapshot = None
    configured = all(
        control.get(field) is not None
        for field in (
            "qualification_cli",
            "qualification_directory",
            "qualification_nonce",
            "projects",
        )
    )
    if configured:
        host = None
        try:
            qualification_cli = Path(control["qualification_cli"]).resolve()
            projects = control["projects"]
            require(
                qualification_cli.is_file()
                and isinstance(projects, list)
                and len(projects) == 2,
                "runtime proof supplied invalid final server cleanup context",
            )
            project = Path(projects[0]).resolve()
            cleanup_env = dict(env)
            cleanup_env.update(
                {
                    "CODESTORY_EMBED_QUALIFICATION_DIR": require_nonempty_string(
                        control["qualification_directory"],
                        "final server cleanup qualification directory",
                    ),
                    "CODESTORY_EMBED_QUALIFICATION_NONCE": require_nonempty_string(
                        control["qualification_nonce"],
                        "final server cleanup qualification nonce",
                    ),
                }
            )
            cleanup_env.pop("CODESTORY_CLI", None)
            archive_sha256 = control.get("plugin_cli_archive_sha256")
            if archive_sha256 is not None:
                cleanup_env["CODESTORY_PLUGIN_CLI_ARCHIVE_SHA256"] = require_sha256(
                    archive_sha256,
                    "final server cleanup archive sha256",
                )
            else:
                cleanup_env.pop("CODESTORY_PLUGIN_CLI_ARCHIVE_SHA256", None)
            host = McpProcess(
                [
                    str(qualification_cli),
                    "serve",
                    "--stdio",
                    "--multi-project",
                    "--refresh",
                    "none",
                ],
                env=cleanup_env,
                cwd=project,
                timeout=args.timeout_secs,
            )
            host.initialize()
            diagnostics = host.engine_diagnostics(project, "final-cleanup-diagnostics")
            raw_snapshot = find_value(diagnostics, "embedding_server")
            require(
                raw_snapshot is not None or not require_final_server,
                "final package server was absent before its clean exit could be authenticated",
            )
            if raw_snapshot is not None:
                require(
                    isinstance(raw_snapshot, dict),
                    "final cleanup diagnostics returned a malformed server snapshot",
                )
                snapshot = server_snapshot(
                    diagnostics,
                    manifest,
                    require_resident=False,
                )
                final_entry = pin_temporary_package_server(
                    control,
                    snapshot["process"],
                    manifest,
                    target_os,
                    "final temporary package embedding server",
                )
        except BaseException as error:
            observation_error = error
        finally:
            if host is not None:
                try:
                    host.close()
                except BaseException as error:
                    host_close_error = error
    elif require_final_server:
        observation_error = ProofFailure(
            "runtime proof omitted final server cleanup context"
        )

    exit_evidence = {}
    waiter_errors = []
    entries = list(waiters)
    control["_waiters"] = []
    wait_budget = native_server_exit_wait_budget(manifest)
    timeout_ms = wait_budget["timeout_ms"]
    final_identity = final_entry.get("identity") if final_entry is not None else None
    entries.sort(
        key=lambda entry: (
            0
            if isinstance(entry, dict) and entry.get("identity") == final_identity
            else 1
        )
    )
    shared_deadline = time.monotonic() + (timeout_ms / 1000)
    superseded_process_count = 0
    for entry in entries:
        waiter = entry.get("waiter") if isinstance(entry, dict) else None
        if not isinstance(waiter, ExactProcessExitWaiter):
            waiter_errors.append("temporary package cleanup waiter was malformed")
            continue
        superseded = final_identity is not None and entry.get("identity") != final_identity
        if superseded:
            superseded_process_count += 1
        require_clean_exit = require_final_server and not superseded
        try:
            process_timeout_ms = remaining_native_server_exit_wait_ms(
                shared_deadline,
                timeout_ms,
            )
            exit_evidence[entry["identity"]] = waiter.wait(
                process_timeout_ms,
                require_clean_exit=require_clean_exit,
            )
        except BaseException as error:
            waiter_errors.append(str(error))
        finally:
            try:
                waiter.close()
            except BaseException as error:
                waiter_errors.append(f"could not close exact process waiter: {error}")

    failures = []
    if observation_error is not None:
        failures.append(f"final server observation failed: {observation_error}")
    if host_close_error is not None:
        failures.append(f"final observational client close failed: {host_close_error}")
    failures.extend(waiter_errors)
    require(not failures, "; ".join(failures))
    if not require_final_server:
        return None
    if final_entry is None:
        return None
    evidence = exit_evidence.get(final_entry["identity"])
    require(isinstance(evidence, dict), "final server cleanup lost its exit evidence")
    return retained_final_native_server_exit_evidence(
        evidence,
        final_entry,
        wait_budget,
        authenticated_process_count=len(entries),
        superseded_process_count=superseded_process_count,
    )


def current_account_identity() -> str:
    if os.name != "nt":
        raw = f"uid:{os.geteuid()}"
        return "account:" + hashlib.sha256(raw.encode("utf-8")).hexdigest()
    completed = subprocess.run(
        ["whoami", "/user", "/fo", "csv", "/nh"],
        text=True,
        capture_output=True,
        timeout=20,
    )
    require(completed.returncode == 0, "could not read current Windows account SID")
    match = re.search(r'"(S-[0-9-]+)"\s*$', completed.stdout.strip())
    require(match is not None, "Windows account command omitted SID")
    raw = f"sid:{match.group(1)}"
    return "account:" + hashlib.sha256(raw.encode("utf-8")).hexdigest()


def opaque_repository_id(project: Path) -> str:
    return "repo:" + hashlib.sha256(str(project.resolve()).encode("utf-8")).hexdigest()
