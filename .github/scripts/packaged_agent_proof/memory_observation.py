"""Five-process resident-memory observation."""

from __future__ import annotations

import argparse
import ctypes
import os
import re
import subprocess
import sys
import time
from pathlib import Path

from .contracts import (
    canonical_sha256,
    load_measurement_protocol,
    require_exact_keys,
    require_nonempty_string,
    require_positive_int,
    selected_qualification_matrix_cell,
    sha256,
)
from .foundation import MEMORY_EVIDENCE_CONTRACT, TARGET_CONTRACTS, ProofFailure, require
from .process_identity import (
    process_start_identity,
    verified_live_executable,
)
from .subprocess_control import McpProcess


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
        require(
            completed.returncode == 0,
            f"could not read physical footprint for process {pid}: "
            f"{completed.stderr.strip()}",
        )
        match = re.search(
            r"^Physical footprint:\s+([^\s]+)",
            completed.stdout,
            re.MULTILINE,
        )
        require(match is not None, f"vmmap omitted the physical footprint for process {pid}")
        return parse_byte_quantity(match.group(1)), "macos_physical_footprint"
    else:
        command = ["ps", "-o", "rss=", "-p", str(pid)]
        scale = 1024
        metric = "rss"
    completed = subprocess.run(command, text=True, capture_output=True, timeout=10)
    require(
        completed.returncode == 0,
        f"could not read RSS for process {pid}: {completed.stderr.strip()}",
    )
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


def _verified_process_set(
    *,
    node_path: Path,
    host_a: McpProcess,
    host_a_start: str,
    host_b: McpProcess,
    host_b_start: str,
    status_a: dict,
    status_b: dict,
    snapshot: dict,
    manifest: dict,
    target_os: str,
) -> tuple[list[dict], set[tuple[int, str]]]:
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
        {"role": "embedding_server", **server_live},
    ]
    identities = {
        (process["pid"], process["process_start_id"]) for process in process_set
    }
    require(
        len(identities) == 5,
        "memory evidence did not identify five distinct live CodeStory processes",
    )
    return process_set, identities


def _sample_process_memory(process_set: list[dict]) -> list[dict]:
    processes = []
    for process in process_set:
        require(
            process_start_identity(process["pid"]) == process["process_start_id"],
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
    return processes


def _memory_sample(
    *,
    repeat: int,
    process_set: list[dict],
    identities: set[tuple[int, str]],
    matrix_cell_id: str,
    matrix_cell: dict,
    workload_id: str,
    target_os: str,
    snapshot: dict,
    boot_id: str,
) -> dict:
    awake_started, inclusive_started, awake_api, inclusive_api = suspend_clock_pair(target_os)
    processes = _sample_process_memory(process_set)
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
            process_start_identity(process["pid"]) == process["process_start_id"],
            f"memory process {process['role']} changed identity during sampling",
        )
    return {
        "sample_id": canonical_sha256(
            {
                "matrix_cell_id": matrix_cell_id,
                "repeat": repeat,
                "identities": sorted(identities),
            }
        ),
        "repeat": repeat,
        "matrix_cell_id": matrix_cell_id,
        "workload_id": workload_id,
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
            "resolution_ns": max(
                1,
                round(time.get_clock_info("monotonic").resolution * 1e9),
            ),
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
    process_set, identities = _verified_process_set(
        node_path=node_path,
        host_a=host_a,
        host_a_start=host_a_start,
        host_b=host_b,
        host_b_start=host_b_start,
        status_a=status_a,
        status_b=status_b,
        snapshot=snapshot,
        manifest=manifest,
        target_os=target_os,
    )
    boot_id = require_nonempty_string(
        snapshot["clock"]["boot_id"],
        "embedding server clock boot_id",
    )
    workload_id = protocol["workloads"]["total_codestory_process_memory"]["workload_id"]
    samples = []
    for repeat in range(1, 4):
        samples.append(
            _memory_sample(
                repeat=repeat,
                process_set=process_set,
                identities=identities,
                matrix_cell_id=matrix_cell_id,
                matrix_cell=matrix_cell,
                workload_id=workload_id,
                target_os=target_os,
                snapshot=snapshot,
                boot_id=boot_id,
            )
        )
        if repeat < 3:
            time.sleep(0.25)
    return {
        "evidence_contract": MEMORY_EVIDENCE_CONTRACT,
        "metric": "total_codestory_process_memory",
        "unit": "bytes",
        "samples": samples,
    }
