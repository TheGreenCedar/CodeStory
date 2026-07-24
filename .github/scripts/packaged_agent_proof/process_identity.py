"""Native process, executable, account, and repository identity."""

from __future__ import annotations

import ctypes
import hashlib
import os
import re
import subprocess
import sys
import time
from pathlib import Path

from .contract_primitives import require_nonempty_string, require_sha256
from .foundation import ProofFailure, require


class _FileTime(ctypes.Structure):
    _fields_ = [
        ("low_date_time", ctypes.c_uint32),
        ("high_date_time", ctypes.c_uint32),
    ]


class _ProcBsdInfo(ctypes.Structure):
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


def _windows_process_start_identity(pid: int) -> str:
    kernel = ctypes.windll.kernel32
    kernel.OpenProcess.argtypes = [ctypes.c_uint32, ctypes.c_int, ctypes.c_uint32]
    kernel.OpenProcess.restype = ctypes.c_void_p
    kernel.GetProcessTimes.argtypes = [
        ctypes.c_void_p,
        ctypes.POINTER(_FileTime),
        ctypes.POINTER(_FileTime),
        ctypes.POINTER(_FileTime),
        ctypes.POINTER(_FileTime),
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
        creation = _FileTime()
        exit_time = _FileTime()
        kernel_time = _FileTime()
        user_time = _FileTime()
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
    creation_ticks = (filetime_ticks // 10 * 10) + 504_911_232_000_000_000
    return f"windows:{creation_ticks}"


def _linux_process_start_identity(pid: int) -> str:
    stat = Path(f"/proc/{pid}/stat").read_text(encoding="utf-8")
    fields = stat.rsplit(") ", 1)
    require(len(fields) == 2, f"/proc/{pid}/stat omitted process start identity")
    process_fields = fields[1].split()
    require(
        len(process_fields) > 19, f"/proc/{pid}/stat omitted process start identity"
    )
    return f"linux:{process_fields[19]}"


def _macos_process_start_identity(pid: int) -> str:
    libproc = ctypes.CDLL("/usr/lib/libproc.dylib", use_errno=True)
    libproc.proc_pidinfo.argtypes = [
        ctypes.c_int,
        ctypes.c_int,
        ctypes.c_uint64,
        ctypes.c_void_p,
        ctypes.c_int,
    ]
    libproc.proc_pidinfo.restype = ctypes.c_int
    info = _ProcBsdInfo()
    expected = ctypes.sizeof(info)
    read = libproc.proc_pidinfo(pid, 3, 0, ctypes.byref(info), expected)
    require(
        read == expected and info.pbi_pid == pid,
        f"could not read complete process start identity for {pid}",
    )
    return f"macos-proc:{info.pbi_start_tvsec}:{info.pbi_start_tvusec}"


def process_start_identity(pid: int) -> str:
    if os.name == "nt":
        return _windows_process_start_identity(pid)
    if sys.platform == "linux":
        return _linux_process_start_identity(pid)
    if sys.platform == "darwin":
        return _macos_process_start_identity(pid)
    completed = subprocess.run(
        ["ps", "-o", "lstart=", "-p", str(pid)],
        text=True,
        capture_output=True,
        timeout=20,
    )
    require(
        completed.returncode == 0, f"could not read process start identity for {pid}"
    )
    return "unix:" + require_nonempty_string(
        completed.stdout.strip(),
        "process start identity",
    )


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


def _macos_executable_descriptor(pid: int) -> int:
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
    return os.open(executable_path, os.O_RDONLY)


def _windows_executable_descriptor(pid: int) -> int:
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
    return os.open(executable_path, os.O_RDONLY | getattr(os, "O_BINARY", 0))


def _executable_descriptor(pid: int, target_os: str) -> int:
    if target_os == "linux":
        return os.open(f"/proc/{pid}/exe", os.O_RDONLY)
    if target_os == "macos":
        return _macos_executable_descriptor(pid)
    require(target_os == "windows", f"unsupported executable-image target {target_os}")
    return _windows_executable_descriptor(pid)


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
    descriptor = _executable_descriptor(pid, target_os)
    digest = hashlib.sha256()
    try:
        for chunk in iter(lambda: os.read(descriptor, 1024 * 1024), b""):
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
            kernel.OpenProcess.argtypes = [
                ctypes.c_uint32,
                ctypes.c_int,
                ctypes.c_uint32,
            ]
            kernel.OpenProcess.restype = ctypes.c_void_p
            self.handle = kernel.OpenProcess(0x00100000 | 0x1000, 0, pid)
            require(
                bool(self.handle), f"could not open exact process {pid} for exit wait"
            )
        try:
            require(
                process_start_identity(pid) == self.expected_start_id,
                f"process {pid} changed identity before exit wait",
            )
        except BaseException:
            self.close()
            raise

    def _wait_windows(self, timeout_ms: int, require_clean_exit: bool) -> int:
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
        return exit_code.value

    def _wait_unix(self, timeout_ms: int) -> None:
        deadline = time.monotonic() + (timeout_ms / 1000)
        while True:
            try:
                current_identity = process_start_identity(self.pid)
            except (FileNotFoundError, ProcessLookupError):
                return
            except ProofFailure:
                try:
                    os.kill(self.pid, 0)
                except ProcessLookupError:
                    return
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

    def wait(self, timeout_ms: int, *, require_clean_exit: bool = True) -> dict:
        require(timeout_ms > 0, "exact process exit wait requires a positive timeout")
        if self.target_os == "windows":
            exit_code = self._wait_windows(timeout_ms, require_clean_exit)
            status = "normal_idle_exit" if exit_code == 0 else "superseded_process_exit"
        else:
            self._wait_unix(timeout_ms)
            exit_code = None
            status = "observed_exit"
        return {
            "status": status,
            "pid": self.pid,
            "process_start_id": self.expected_start_id,
            "exit_code": exit_code,
            "clean_exit_required": require_clean_exit,
            "timeout_ms": timeout_ms,
        }

    def close(self) -> None:
        if self.handle is not None:
            kernel = ctypes.windll.kernel32
            kernel.CloseHandle.argtypes = [ctypes.c_void_p]
            kernel.CloseHandle(self.handle)
            self.handle = None


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
