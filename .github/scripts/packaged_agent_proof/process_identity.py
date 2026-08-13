"""Native process, executable, account, and repository identity."""

from __future__ import annotations

import ctypes
import hashlib
import os
import re
import subprocess
import sys
import time
from enum import Enum
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


class _UnixExitProbeState(Enum):
    GONE_OR_REUSED = "gone_or_reused"
    MATCHING = "matching"
    UNKNOWN = "unknown"


def _windows_handle_process_identity(
    kernel,
    handle: int,
    pid: int,
) -> tuple[str, int, bool]:
    """Read one held Windows process object's identity and current exit code."""
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
        bool(kernel.GetExitCodeProcess(handle, ctypes.byref(exit_code))),
        f"could not read process exit state for {pid}",
    )
    filetime_ticks = (creation.high_date_time << 32) | creation.low_date_time
    creation_ticks = (filetime_ticks // 10 * 10) + 504_911_232_000_000_000
    exit_time_is_zero = (
        exit_time.low_date_time == 0 and exit_time.high_date_time == 0
    )
    return f"windows:{creation_ticks}", exit_code.value, exit_time_is_zero


def _windows_process_start_identity(pid: int) -> str:
    kernel = ctypes.windll.kernel32
    kernel.OpenProcess.argtypes = [ctypes.c_uint32, ctypes.c_int, ctypes.c_uint32]
    kernel.OpenProcess.restype = ctypes.c_void_p
    kernel.CloseHandle.argtypes = [ctypes.c_void_p]
    handle = kernel.OpenProcess(0x1000, 0, pid)
    require(bool(handle), f"could not open process {pid} for start identity")
    try:
        identity, exit_code, exit_time_is_zero = _windows_handle_process_identity(
            kernel,
            handle,
            pid,
        )
        require(
            exit_code == 259 and exit_time_is_zero,
            f"process {pid} was not running during start-identity inspection",
        )
        return identity
    finally:
        kernel.CloseHandle(handle)


def _linux_process_start_identity(pid: int) -> str:
    stat = Path(f"/proc/{pid}/stat").read_text(encoding="utf-8")
    fields = stat.rsplit(") ", 1)
    require(len(fields) == 2, f"/proc/{pid}/stat omitted process start identity")
    process_fields = fields[1].split()
    require(
        len(process_fields) > 19, f"/proc/{pid}/stat omitted process start identity"
    )
    return f"linux:{process_fields[19]}"


def linux_terminated_state(stat: str) -> str | None:
    """The terminated state named in one `/proc/<pid>/stat`, or None.

    A phrase, not a sentence: the caller owns naming the process it asked about.
    """
    fields = stat.rsplit(") ", 1)
    if len(fields) != 2:
        return None
    process_fields = fields[1].split()
    # `Z` is exited-but-unreaped; `X` and `x` are the kernel's dead states.
    if process_fields and process_fields[0] in {"Z", "X", "x"}:
        return (
            f"is in state {process_fields[0]}: it has terminated and is waiting"
            " to be reaped"
        )
    return None


def terminated_process_state(pid: int) -> str | None:
    """How ``pid`` is provably no longer running, or None if it may still be.

    Only Linux needs this. A process that has exited but has not yet been reaped
    keeps a readable ``/proc/<pid>/stat`` whose start time never changes, so a
    liveness probe built on start identity alone calls a dead process running
    until its parent reaps it. macOS ``proc_pidinfo`` already fails outright for
    a zombie, and Windows ``GetExitCodeProcess`` already reports its real exit
    code, so both answer correctly without this.

    Observational only: an unreadable or unparsable ``/proc`` entry answers
    None, because "cannot tell" must never be reported as "has exited".
    """
    if sys.platform != "linux":
        return None
    try:
        stat = Path(f"/proc/{pid}/stat").read_text(encoding="utf-8")
    except (FileNotFoundError, ProcessLookupError):
        return "no longer exists"
    except OSError:
        return None
    return linux_terminated_state(stat)


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
    def __init__(
        self,
        pid: int,
        expected_start_id: str,
        target_os: str,
        *,
        allow_already_exited: bool = False,
    ):
        self.pid = pid
        self.expected_start_id = require_native_process_start_identity(
            expected_start_id,
            target_os,
            f"process {pid} expected exit-wait identity",
        )
        self.target_os = target_os
        self.handle = None
        self._already_exited_reason = None
        self._unix_probe_state = None
        self._unix_probe_detail = None
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
            kernel.GetLastError.restype = ctypes.c_uint32
            self.handle = kernel.OpenProcess(0x00100000 | 0x1000, 0, pid)
            if not self.handle:
                error_code = int(kernel.GetLastError())
                if allow_already_exited and error_code == 87:
                    self._already_exited_reason = (
                        f"pid {pid} no longer names a Windows process"
                    )
                    return
                raise ProofFailure(
                    f"could not open exact process {pid} for exit wait:"
                    f" Windows error {error_code}"
                )
            try:
                observed, exit_code, exit_time_is_zero = _windows_handle_process_identity(
                    kernel,
                    self.handle,
                    pid,
                )
                if observed != self.expected_start_id:
                    if allow_already_exited:
                        self._already_exited_reason = (
                            f"pid {pid} now carries start identity {observed},"
                            f" replacing {self.expected_start_id}"
                        )
                        self.close()
                        return
                    raise ProofFailure(
                        f"process {pid} changed identity before exit wait"
                    )
                if exit_code != 259 or not exit_time_is_zero:
                    if allow_already_exited:
                        self._already_exited_reason = (
                            f"exact Windows process {pid} already exited with"
                            f" code {exit_code}"
                        )
                        return
                    raise ProofFailure(
                        f"process {pid} was not running before exit wait"
                    )
            except BaseException:
                self.close()
                raise
            return
        try:
            state, reason = self._observe_unix_identity()
        except BaseException:
            self.close()
            raise
        if state is _UnixExitProbeState.GONE_OR_REUSED:
            if allow_already_exited:
                self._already_exited_reason = reason
                return
            raise ProofFailure(
                f"exact process {pid} {reason} before exit wait"
            )

        # A transiently unreadable Unix identity is not exit evidence and is
        # not a constructor failure. Preserve it so observational callers can
        # fail closed, while the bounded waiter can reclassify until its
        # deadline. MATCHING needs no additional constructor work.

    def _classify_unix_identity(self) -> tuple[_UnixExitProbeState, str | None]:
        terminated = terminated_process_state(self.pid)
        if terminated is not None:
            return _UnixExitProbeState.GONE_OR_REUSED, terminated
        try:
            observed = process_start_identity(self.pid)
        except (FileNotFoundError, ProcessLookupError):
            return _UnixExitProbeState.GONE_OR_REUSED, "no longer exists"
        except (ProofFailure, OSError) as error:
            # An inspection error is not exit evidence. Confirm only process
            # absence. A present but unreadable identity stays explicitly
            # unknown so a bounded waiter can keep polling without calling it
            # matching or gone.
            try:
                os.kill(self.pid, 0)
            except ProcessLookupError:
                return _UnixExitProbeState.GONE_OR_REUSED, "no longer exists"
            except (PermissionError, OSError) as liveness_error:
                return (
                    _UnixExitProbeState.UNKNOWN,
                    f"could not prove exact process {self.pid} exited after"
                    f" identity inspection failed ({error}); liveness probe"
                    f" also failed ({liveness_error})",
                )
            return (
                _UnixExitProbeState.UNKNOWN,
                f"could not inspect exact process {self.pid} start identity"
                f" while the PID remains present: {error}",
            )
        if observed != self.expected_start_id:
            return (
                _UnixExitProbeState.GONE_OR_REUSED,
                f"now carries start identity {observed}, replacing"
                f" {self.expected_start_id}",
            )
        return _UnixExitProbeState.MATCHING, None

    def _record_unix_identity(
        self,
        classification: tuple[_UnixExitProbeState, str | None],
    ) -> tuple[_UnixExitProbeState, str | None]:
        self._unix_probe_state, self._unix_probe_detail = classification
        return classification

    def _observe_unix_identity(self) -> tuple[_UnixExitProbeState, str | None]:
        return self._record_unix_identity(self._classify_unix_identity())

    def exited(self) -> bool:
        """Whether the exact held process is proven gone, never merely unreadable."""
        if self._already_exited_reason is not None:
            return True
        if self.target_os == "windows":
            kernel = ctypes.windll.kernel32
            kernel.WaitForSingleObject.argtypes = [ctypes.c_void_p, ctypes.c_uint32]
            kernel.WaitForSingleObject.restype = ctypes.c_uint32
            result = kernel.WaitForSingleObject(self.handle, 0)
            if result == 0:
                return True
            if result == 258:
                return False
            raise ProofFailure(
                f"exact process {self.pid} exit probe failed with result {result}"
            )
        state, reason = self._observe_unix_identity()
        if state is _UnixExitProbeState.MATCHING:
            return False
        if state is _UnixExitProbeState.GONE_OR_REUSED:
            self._already_exited_reason = reason
            return True
        raise ProofFailure(reason)

    def _windows_expired_wait_state(self, kernel) -> str:
        exit_code = ctypes.c_uint32()
        if not kernel.GetExitCodeProcess(self.handle, ctypes.byref(exit_code)):
            return "in an unreadable exit state"
        if exit_code.value == 259:
            return "still running"
        return f"exited with code {exit_code.value} only after the wait expired"

    def _wait_windows(self, timeout_ms: int, require_clean_exit: bool) -> int:
        kernel = ctypes.windll.kernel32
        kernel.WaitForSingleObject.argtypes = [ctypes.c_void_p, ctypes.c_uint32]
        kernel.WaitForSingleObject.restype = ctypes.c_uint32
        kernel.GetExitCodeProcess.argtypes = [
            ctypes.c_void_p,
            ctypes.POINTER(ctypes.c_uint32),
        ]
        kernel.GetExitCodeProcess.restype = ctypes.c_int
        started = time.monotonic()
        result = kernel.WaitForSingleObject(self.handle, timeout_ms)
        if result == 258:
            # A timed-out receipt must carry enough evidence to refute a
            # vacuous pass: which exact process was held, how long it was
            # actually held, and what state it was left in.
            waited_ms = int((time.monotonic() - started) * 1000)
            raise ProofFailure(
                f"exact process {self.pid} (start identity"
                f" {self.expected_start_id}) did not exit within {timeout_ms}ms:"
                f" waited {waited_ms}ms and left it"
                f" {self._windows_expired_wait_state(kernel)}"
            )
        require(
            result == 0,
            f"exact process {self.pid} exit wait failed with result {result}",
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

    def _wait_unix(
        self,
        timeout_ms: int,
        *,
        now=None,
        sleep=None,
        classify=None,
    ) -> None:
        now = time.monotonic if now is None else now
        sleep = time.sleep if sleep is None else sleep
        classify = self._classify_unix_identity if classify is None else classify

        def observe() -> tuple[_UnixExitProbeState, str | None]:
            return self._record_unix_identity(classify())

        started = now()
        deadline = started + (timeout_ms / 1000)

        def timeout(
            current: float,
            state: _UnixExitProbeState,
            detail: str | None,
        ) -> ProofFailure:
            waited_ms = int(max(0, current - started) * 1000)
            if state is _UnixExitProbeState.GONE_OR_REUSED:
                evidence = "exit was first observed only after the wait expired"
            elif state is _UnixExitProbeState.MATCHING:
                evidence = "left it still running with its start identity unchanged"
            else:
                evidence = f"left its exact identity uncertain: {detail}"
            return ProofFailure(
                f"exact process {self.pid} (start identity"
                f" {self.expected_start_id}) did not exit within"
                f" {timeout_ms}ms: waited {waited_ms}ms and {evidence}"
            )

        # Phase one classifies the exact identity once without a deadline gate.
        # A process already gone or a PID already reused satisfies the fence,
        # even with a zero budget or a slow initial native inspection.
        state, detail = observe()
        if state is _UnixExitProbeState.GONE_OR_REUSED:
            self._already_exited_reason = detail
            return

        # Phase two is strictly deadline-gated. No later probe begins after an
        # overshooting sleep, and an exit first learned by a slow probe at or
        # after the deadline cannot satisfy the bounded fence. Unknown identity
        # is nonterminal but remains distinct from a proven matching process so
        # timeout evidence fails closed without claiming it was still running.
        while True:
            current = now()
            if current >= deadline:
                raise timeout(current, state, detail)
            sleep(0.01)
            current = now()
            if current >= deadline:
                raise timeout(current, state, detail)
            state, detail = observe()
            current = now()
            if current >= deadline:
                raise timeout(current, state, detail)
            if state is _UnixExitProbeState.GONE_OR_REUSED:
                self._already_exited_reason = detail
                return

    def wait(self, timeout_ms: int, *, require_clean_exit: bool = True) -> dict:
        require(timeout_ms > 0, "exact process exit wait requires a positive timeout")
        if self._already_exited_reason is not None:
            exit_code = None
            status = "observed_exit"
        elif self.target_os == "windows":
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
