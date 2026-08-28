"""Publication crash-exit fence self-tests.

The product records ``crash_server`` as accepted before aborting. These tests
pin the harness order after that receipt: observe the exact predecessor exit,
keep the paused publication candidate alive in the same loop, and only then
issue one immutable replacement-worker query.
"""

from __future__ import annotations

import hashlib
import tempfile
from pathlib import Path
from types import SimpleNamespace
from unittest.mock import Mock, patch

from . import process_identity, publication_protocol
from .event_producer_liveness import EventProducer, NativeProcessProducer
from .foundation import ProofFailure, require


class _Clock:
    def __init__(self) -> None:
        self.now = 0.0
        self.sleeps: list[float] = []

    def monotonic(self) -> float:
        return self.now

    def sleep(self, seconds: float) -> None:
        require(seconds > 0, "publication crash-exit self-test slept nonpositively")
        self.sleeps.append(seconds)
        self.now += seconds


def _v3_search_evidence_rank_test() -> None:
    query = "qualification_anchor_00"
    with tempfile.TemporaryDirectory() as temporary:
        project = Path(temporary) / "project"
        project.mkdir()
        (project / "lib.rs").write_text(
            "pub fn qualification_anchor_01() {}\n"
            "pub fn qualification_anchor_00() {}\n",
            encoding="utf-8",
        )
        payload = {
            "kind": "complete",
            "schema_version": 3,
            "identity": {
                "packet_id": "packet-publication-self-test",
                "request_id": "request-publication-self-test",
                "question_sha256": hashlib.sha256(query.encode("utf-8")).hexdigest()
            },
            "publication": {
                "core": {
                    "project_id": "project-publication-self-test",
                    "generation_id": "core-generation-publication-self-test",
                    "run_id": "core-run-publication-self-test",
                },
                "retrieval": {
                    "core_generation_id": "core-generation-publication-self-test",
                    "core_run_id": "core-run-publication-self-test",
                    "retrieval_generation": "retrieval-publication-self-test",
                    "retrieval_input_sha256": "a" * 64,
                    "semantic_generation": "semantic-publication-self-test",
                },
            },
            "status": "available",
            "evidence": [
                {
                    "identity": {"evidence_id": "evidence-wrong-line"},
                    "path": "lib.rs",
                    "symbol_id": None,
                    "start_line": 1,
                    "end_line": 1,
                    "excerpt": query,
                },
                {
                    "identity": {"evidence_id": "evidence-right-line"},
                    "path": "lib.rs",
                    "symbol_id": None,
                    "start_line": 2,
                    "end_line": 2,
                    "excerpt": None,
                },
            ],
            "gaps": [],
            "continuation": None,
            "retrieval": {
                "state": "full",
                "generation_id": "retrieval-publication-self-test",
            },
            "diagnostics": {"availability": "unavailable"},
        }
        require(
            publication_protocol.rank_v3_search_evidence(
                project, payload, query, query
            )
            == 2,
            "v3 qualification search changed its evidence rank",
        )

        hostile = dict(payload)
        hostile["evidence"] = [
            {
                "identity": {"evidence_id": "evidence-escaped"},
                "path": "../outside.rs",
                "symbol_id": None,
                "start_line": 1,
                "end_line": 1,
                "excerpt": query,
            }
        ]
        try:
            publication_protocol.rank_v3_search_evidence(
                project, hostile, query, query
            )
        except (OSError, ProofFailure):
            pass
        else:
            raise ProofFailure("v3 qualification search accepted an escaped source path")


class _OversleepClock(_Clock):
    def sleep(self, seconds: float) -> None:
        super().sleep(seconds)
        self.now += 0.2


class _ScriptedPredecessor(NativeProcessProducer):
    def __init__(self, exits: list[bool]) -> None:
        super().__init__(
            4242,
            "self-test-predecessor-start",
            "the self-test predecessor",
            "exiting after accepting crash_server",
        )
        require(bool(exits), "scripted predecessor needs at least one state")
        self._exits = list(exits)
        self.probes = 0

    def exited(self) -> bool:
        state = self._exits[min(self.probes, len(self._exits) - 1)]
        self.probes += 1
        return state


class _ScriptedCandidate(EventProducer):
    def __init__(self, *, exit_on_probe: int | None = None) -> None:
        super().__init__(
            "the publication candidate process",
            "remaining paused before manifest commit",
        )
        self.exit_on_probe = exit_on_probe
        self.probes = 0

    def exited(self) -> bool:
        self.probes += 1
        return self.exit_on_probe is not None and self.probes >= self.exit_on_probe

    def termination(self) -> str:
        return "exited with code 17 stdout_sha256=self-test stderr_sha256=self-test"


class _ScriptedExitWaiter:
    def __init__(self, predecessor: _ScriptedPredecessor) -> None:
        self.predecessor = predecessor
        self.closed = False

    def exited(self) -> bool:
        return self.predecessor.exited()

    def close(self) -> None:
        self.closed = True


def _accepted_event(
    pid: int = 4242,
    start: object = "self-test-predecessor-start",
) -> dict:
    return {
        "sequence": 2,
        "action": "crash_server",
        "status": "accepted",
        "snapshot": {
            "process": {
                "pid": pid,
                "process_start_id": start,
            }
        },
    }


def _drive_replacement(
    predecessor: NativeProcessProducer,
    candidate: EventProducer,
    *,
    allowance_secs: float,
    clock: _Clock | None = None,
    invocations: list[tuple[float, int | None]] | None = None,
) -> tuple[_Clock, list[tuple[float, int | None]]]:
    clock = clock or _Clock()
    invocations = invocations if invocations is not None else []

    def replacement(*_args, **_kwargs) -> dict:
        invocations.append((clock.now, getattr(predecessor, "probes", None)))
        return {}

    waiter = _ScriptedExitWaiter(predecessor)

    def exit_waiter(pid, start, _target_os, *, allow_already_exited=False):
        require(
            pid == predecessor.pid
            and start == predecessor.process_start_id
            and allow_already_exited is True,
            "the publication fence did not construct an already-exited-tolerant"
            " waiter for the crash event's exact identity",
        )
        return waiter

    with (
        patch.object(
            publication_protocol,
            "server_producer_from_control_event",
            return_value=predecessor,
        ),
        patch.object(
            publication_protocol,
            "ExactProcessExitWaiter",
            side_effect=exit_waiter,
        ),
        patch.object(
            publication_protocol,
            "run_embedding_qualification_query_worker",
            replacement,
        ),
        patch.object(publication_protocol.time, "monotonic", clock.monotonic),
        patch.object(publication_protocol.time, "sleep", clock.sleep),
    ):
        publication_protocol.run_publication_replacement_worker(
            Path("/self-test/codestory-cli"),
            {},
            Path("/self-test/project"),
            Path("/self-test/private"),
            "self-test-nonce",
            crash_event=_accepted_event(),
            candidate_producer=candidate,
            executable_sha256="a" * 64,
            timeout=5,
            crash_exit_allowance_secs=allowance_secs,
        )
    require(waiter.closed, "publication crash-exit waiter leaked its native handle")
    return clock, invocations


def _delayed_exit_blocks_then_invokes_exactly_one_replacement() -> None:
    predecessor = _ScriptedPredecessor([False, False, True])
    candidate = _ScriptedCandidate()
    clock, invocations = _drive_replacement(
        predecessor,
        candidate,
        allowance_secs=1.0,
    )
    require(
        predecessor.probes == 3
        and len(clock.sleeps) == 2
        and invocations == [(0.1, 3)],
        "a delayed predecessor exit did not block one replacement until the"
        f" exact exit was observed: sleeps={clock.sleeps} calls={invocations}",
    )


def _accepted_event_pins_the_process_that_answered() -> None:
    event = _accepted_event()
    predecessor = publication_protocol.server_producer_from_control_event(
        event,
        "exiting after accepting the self-test crash",
    )
    require(
        isinstance(predecessor, NativeProcessProducer)
        and predecessor.pid == 4242
        and predecessor.process_start_id == "self-test-predecessor-start",
        "an accepted crash event did not pin its own server PID and process-start"
        " identity",
    )


def _immediate_exit_adds_no_delay() -> None:
    predecessor = _ScriptedPredecessor([True])
    clock, invocations = _drive_replacement(
        predecessor,
        _ScriptedCandidate(),
        allowance_secs=1.0,
    )
    require(
        clock.sleeps == [] and invocations == [(0.0, 1)],
        "an already-gone predecessor delayed or duplicated the replacement",
    )


def _never_exit_fails_by_identity_without_replacement() -> None:
    predecessor = _ScriptedPredecessor([False])
    candidate = _ScriptedCandidate()
    clock = _Clock()
    invocations: list[object] = []
    waiter = _ScriptedExitWaiter(predecessor)
    try:
        with (
            patch.object(
                publication_protocol,
                "server_producer_from_control_event",
                return_value=predecessor,
            ),
            patch.object(
                publication_protocol,
                "ExactProcessExitWaiter",
                return_value=waiter,
            ),
            patch.object(
                publication_protocol,
                "run_embedding_qualification_query_worker",
                side_effect=lambda *_args, **_kwargs: invocations.append(object()),
            ),
            patch.object(publication_protocol.time, "monotonic", clock.monotonic),
            patch.object(publication_protocol.time, "sleep", clock.sleep),
        ):
            publication_protocol.run_publication_replacement_worker(
                Path("/self-test/cli"),
                {},
                Path("/self-test/project"),
                Path("/self-test/private"),
                "self-test-nonce",
                crash_event=_accepted_event(),
                candidate_producer=candidate,
                executable_sha256="a" * 64,
                timeout=5,
                crash_exit_allowance_secs=0.11,
            )
    except ProofFailure as error:
        message = str(error)
        require(
            message
            == (
                f"{publication_protocol.PUBLICATION_CRASH_EXIT_TIMEOUT}: pid 4242"
                " (start identity self-test-predecessor-start) remained live for"
                " 110ms after accepting crash_server; no replacement worker was"
                " invoked"
            ),
            f"the bounded crash-exit failure lost its exact identity: {message}",
        )
    else:
        raise ProofFailure("a predecessor that never exited started a replacement")
    require(
        not invocations and waiter.closed and 0.11 <= clock.now < 0.12,
        "the never-exit path was unbounded or invoked a replacement",
    )


def _candidate_exit_fails_before_the_predecessor_probe() -> None:
    predecessor = _ScriptedPredecessor([False])
    candidate = _ScriptedCandidate(exit_on_probe=1)
    try:
        _drive_replacement(predecessor, candidate, allowance_secs=1.0)
    except ProofFailure as error:
        message = str(error)
        require(
            "the publication candidate process" in message
            and "exited with code 17" in message
            and "pid 4242 (start identity self-test-predecessor-start)" in message
            and "no replacement worker was invoked" in message,
            f"the candidate exit was not attributed immediately: {message}",
        )
    else:
        raise ProofFailure("a dead paused candidate allowed a replacement query")
    require(
        predecessor.probes == 0,
        "the crash fence probed the predecessor before noticing candidate exit",
    )


def _candidate_exit_wins_the_same_poll_as_predecessor_exit() -> None:
    predecessor = _ScriptedPredecessor([True])
    candidate = _ScriptedCandidate(exit_on_probe=2)
    try:
        _drive_replacement(predecessor, candidate, allowance_secs=1.0)
    except ProofFailure as error:
        require(
            "the publication candidate process" in str(error)
            and "no replacement worker was invoked" in str(error),
            f"same-poll candidate exit was not attributed: {error}",
        )
    else:
        raise ProofFailure(
            "predecessor exit hid a simultaneous paused-candidate exit"
        )
    require(
        predecessor.probes == 1 and candidate.probes == 2,
        "same-poll candidate regression did not reach the settling recheck",
    )


def _late_exit_after_oversleep_fails_without_a_final_probe() -> None:
    predecessor = _ScriptedPredecessor([False, True])
    clock = _OversleepClock()
    invocations: list[tuple[float, int | None]] = []
    try:
        _drive_replacement(
            predecessor,
            _ScriptedCandidate(),
            allowance_secs=0.11,
            clock=clock,
            invocations=invocations,
        )
    except ProofFailure as error:
        require(
            str(error).startswith(
                f"{publication_protocol.PUBLICATION_CRASH_EXIT_TIMEOUT}: pid 4242"
            )
            and "no replacement worker was invoked" in str(error),
            f"an overslept late exit returned the wrong failure: {error}",
        )
    else:
        raise ProofFailure("a predecessor exit observed after the bound passed")
    require(
        predecessor.probes == 1 and not invocations and clock.now > 0.11,
        "the oversleep path probed after its deadline or invoked a replacement",
    )


def _unix_waiter_with(
    *,
    expected_start: str = "linux:1",
) -> process_identity.ExactProcessExitWaiter:
    waiter = object.__new__(process_identity.ExactProcessExitWaiter)
    waiter.pid = 4242
    waiter.expected_start_id = expected_start
    waiter.target_os = "linux"
    waiter.handle = None
    waiter._already_exited_reason = None
    return waiter


def _fake_windows_kernel(*, handle: int = 73, last_error: int = 0):
    return SimpleNamespace(
        OpenProcess=Mock(return_value=handle),
        GetLastError=Mock(return_value=last_error),
        CloseHandle=Mock(return_value=1),
        WaitForSingleObject=Mock(return_value=258),
    )


def _windows_waiter(
    kernel,
    *,
    observed: tuple[str, int, bool] | BaseException,
) -> process_identity.ExactProcessExitWaiter:
    identity_probe = (
        patch.object(
            process_identity,
            "_windows_handle_process_identity",
            side_effect=observed,
        )
        if isinstance(observed, BaseException)
        else patch.object(
            process_identity,
            "_windows_handle_process_identity",
            return_value=observed,
        )
    )
    with (
        patch.object(process_identity.os, "name", "nt"),
        patch.object(
            process_identity.ctypes,
            "windll",
            SimpleNamespace(kernel32=kernel),
            create=True,
        ),
        identity_probe,
    ):
        return process_identity.ExactProcessExitWaiter(
            4242,
            "windows:1",
            "windows",
            allow_already_exited=True,
        )


def _pid_reuse_is_a_proven_predecessor_exit() -> None:
    unix_waiter = _unix_waiter_with()
    with (
        patch.object(process_identity, "terminated_process_state", return_value=None),
        patch.object(
            process_identity,
            "process_start_identity",
            return_value="linux:2",
        ),
    ):
        require(
            unix_waiter.exited(),
            "the Unix exact waiter did not classify PID reuse as predecessor exit",
        )

    windows_kernel = _fake_windows_kernel()
    windows_waiter = _windows_waiter(
        windows_kernel,
        observed=("windows:2", 259, True),
    )
    require(
        windows_waiter.exited()
        and windows_waiter.handle is None
        and windows_kernel.CloseHandle.call_count == 1,
        "the Windows held-handle waiter did not classify PID reuse as predecessor"
        " exit",
    )

    candidate = _ScriptedCandidate()
    invocations: list[str] = []
    expected_start = (
        "windows:1"
        if publication_protocol.os.name == "nt"
        else (
            "macos-proc:1:1"
            if publication_protocol.sys.platform == "darwin"
            else "linux:1"
        )
    )
    with (
        patch.object(
            publication_protocol,
            "ExactProcessExitWaiter",
            return_value=_ScriptedExitWaiter(_ScriptedPredecessor([True])),
        ),
        patch.object(
            publication_protocol,
            "run_embedding_qualification_query_worker",
            side_effect=lambda *_args, **_kwargs: invocations.append("replacement"),
        ),
    ):
        publication_protocol.run_publication_replacement_worker(
            Path("/self-test/cli"),
            {},
            Path("/self-test/project"),
            Path("/self-test/private"),
            "self-test-nonce",
            crash_event=_accepted_event(start=expected_start),
            candidate_producer=candidate,
            executable_sha256="a" * 64,
            timeout=5,
            crash_exit_allowance_secs=1.0,
        )
    require(
        invocations == ["replacement"],
        "a reused PID with a different start identity did not release exactly"
        " one replacement",
    )


def _unobservable_identity_fails_closed_without_replacement() -> None:
    unix_waiter = _unix_waiter_with()
    with (
        patch.object(process_identity, "terminated_process_state", return_value=None),
        patch.object(
            process_identity,
            "process_start_identity",
            side_effect=OSError("self-test identity denied"),
        ),
        patch.object(process_identity.os, "kill", return_value=None),
    ):
        try:
            unix_waiter.exited()
        except ProofFailure as error:
            require(
                "PID remains present" in str(error),
                f"the Unix waiter returned the wrong unobservable failure: {error}",
            )
        else:
            raise ProofFailure("the Unix waiter treated unreadable identity as exit")

    windows_kernel = _fake_windows_kernel()
    try:
        _windows_waiter(
            windows_kernel,
            observed=ProofFailure("self-test held-handle identity denied"),
        )
    except ProofFailure as error:
        require(
            "held-handle identity denied" in str(error)
            and windows_kernel.CloseHandle.call_count == 1,
            f"the Windows waiter did not fail closed and release its handle: {error}",
        )
    else:
        raise ProofFailure("the Windows waiter treated unreadable identity as exit")

    denied_kernel = _fake_windows_kernel(handle=0, last_error=5)
    try:
        _windows_waiter(
            denied_kernel,
            observed=("windows:1", 259, True),
        )
    except ProofFailure as error:
        require(
            "Windows error 5" in str(error),
            f"Windows access denial returned the wrong fail-closed error: {error}",
        )
    else:
        raise ProofFailure("Windows access denial was accepted as process exit")

    candidate = _ScriptedCandidate()
    invocations: list[object] = []
    expected_start = (
        "windows:1"
        if publication_protocol.os.name == "nt"
        else (
            "macos-proc:1:1"
            if publication_protocol.sys.platform == "darwin"
            else "linux:1"
        )
    )
    try:
        with (
            patch.object(
                publication_protocol,
                "ExactProcessExitWaiter",
                side_effect=ProofFailure(
                    "could not inspect exact process 4242 start identity while the"
                    " PID remains present: self-test identity denied"
                ),
            ),
            patch.object(
                publication_protocol,
                "run_embedding_qualification_query_worker",
                side_effect=lambda *_args, **_kwargs: invocations.append(object()),
            ),
        ):
            publication_protocol.run_publication_replacement_worker(
                Path("/self-test/cli"),
                {},
                Path("/self-test/project"),
                Path("/self-test/private"),
                "self-test-nonce",
                crash_event=_accepted_event(start=expected_start),
                candidate_producer=candidate,
                executable_sha256="a" * 64,
                timeout=5,
                crash_exit_allowance_secs=1.0,
            )
    except ProofFailure as error:
        require(
            "could not inspect exact process 4242 start identity" in str(error)
            and "PID remains present" in str(error),
            f"an unobservable process did not fail closed by identity: {error}",
        )
    else:
        raise ProofFailure("an unobservable predecessor allowed a replacement")
    require(not invocations, "an unobservable predecessor invoked a replacement")


def _missing_native_identity_fails_closed() -> None:
    malformed = (
        None,
        {"process": None},
        {"process": {"pid": 4242}},
        {"process": {"pid": True, "process_start_id": "self-test-start"}},
    )
    for snapshot in malformed:
        invocations: list[object] = []
        try:
            with patch.object(
                publication_protocol,
                "run_embedding_qualification_query_worker",
                side_effect=lambda *_args, **_kwargs: invocations.append(object()),
            ):
                event = _accepted_event()
                event["snapshot"] = snapshot
                publication_protocol.run_publication_replacement_worker(
                    Path("/self-test/cli"),
                    {},
                    Path("/self-test/project"),
                    Path("/self-test/private"),
                    "self-test-nonce",
                    crash_event=event,
                    candidate_producer=_ScriptedCandidate(),
                    executable_sha256="a" * 64,
                    timeout=5,
                    crash_exit_allowance_secs=1.0,
                )
        except ProofFailure as error:
            require(
                str(error).startswith(
                    publication_protocol.PUBLICATION_CRASH_IDENTITY_MISSING + ":"
                ),
                f"missing crash identity did not fail by name: {error}",
            )
        else:
            raise ProofFailure(
                f"missing crash identity started a replacement: {snapshot}"
            )
        require(
            not invocations,
            "missing crash identity invoked a replacement worker",
        )


def _replacement_requires_the_durable_accepted_event() -> None:
    invocations: list[object] = []
    event = _accepted_event()
    event["status"] = "completed"
    try:
        with patch.object(
            publication_protocol,
            "run_embedding_qualification_query_worker",
            side_effect=lambda *_args, **_kwargs: invocations.append(object()),
        ):
            publication_protocol.run_publication_replacement_worker(
                Path("/self-test/cli"),
                {},
                Path("/self-test/project"),
                Path("/self-test/private"),
                "self-test-nonce",
                crash_event=event,
                candidate_producer=_ScriptedCandidate(),
                executable_sha256="a" * 64,
                timeout=5,
                crash_exit_allowance_secs=1.0,
            )
    except ProofFailure as error:
        require(
            str(error).startswith(
                publication_protocol.PUBLICATION_CRASH_NOT_ACCEPTED + ":"
            ),
            f"a non-accepted crash event did not fail by name: {error}",
        )
    else:
        raise ProofFailure("replacement started without an accepted crash receipt")
    require(not invocations, "a non-accepted crash event invoked a replacement")


def run_publication_crash_exit_self_tests() -> None:
    _v3_search_evidence_rank_test()
    _delayed_exit_blocks_then_invokes_exactly_one_replacement()
    _accepted_event_pins_the_process_that_answered()
    _immediate_exit_adds_no_delay()
    _never_exit_fails_by_identity_without_replacement()
    _candidate_exit_fails_before_the_predecessor_probe()
    _candidate_exit_wins_the_same_poll_as_predecessor_exit()
    _late_exit_after_oversleep_fails_without_a_final_probe()
    _pid_reuse_is_a_proven_predecessor_exit()
    _unobservable_identity_fails_closed_without_replacement()
    _missing_native_identity_fails_closed()
    _replacement_requires_the_durable_accepted_event()
