"""Resident-memory and awake-clock sampling primitives."""

from __future__ import annotations

import ctypes
import os
import re
import subprocess
import sys
import time

from .foundation import ProofFailure, require


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
        require(
            match is not None, f"vmmap omitted the physical footprint for process {pid}"
        )
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
        raise ProofFailure(
            f"invalid RSS for process {pid}: {completed.stdout!r}"
        ) from exc


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
    require(
        target_os == "windows", f"unsupported qualification clock target {target_os}"
    )
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
