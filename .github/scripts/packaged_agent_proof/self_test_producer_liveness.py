"""Self-tests for qualification producer liveness attribution.

The defect these tests pin: a qualification wait without a liveness argument
cannot tell a dead producer from a slow one, so every producer failure spends
the whole proof budget and reports only that a record never arrived. Each test
here fails if that behaviour comes back.
"""

from __future__ import annotations

import ast
import json
import os
import subprocess
import sys
import tempfile
import time
from pathlib import Path

from .event_producer_liveness import (
    ChildProcessProducer,
    NativeProcessProducer,
    ObservationalProducer,
    ProducerGroup,
    UnobservedProducer,
)
from .foundation import REPOSITORY_ROOT, ProofFailure, require
from .process_identity import (
    linux_terminated_state,
    process_start_identity,
    terminated_process_state,
)
from .publication_protocol import (
    jsonl_log_state,
    send_server_qualification_control,
    server_producer_from_control_event,
    wait_for_jsonl_event,
)

# Far longer than any of these tests may take, and far shorter than the proof
# budget they stand in for. A wait that consults its producer fails in
# milliseconds; a wait that ignores it burns the whole timeout, which is the
# regression being pinned. The real harness passes a 900s budget, but pinning
# the property does not require paying it: reproducing the defect must not cost
# every proof workflow that runs `--self-test` a 15-minute hang before the
# assertion can fire.
_UNREACHED_TIMEOUT_SECS = 30
_ATTRIBUTION_BUDGET_SECS = 8


def _dead_process() -> subprocess.Popen:
    process = subprocess.Popen(
        [sys.executable, "-c", "import sys; sys.stderr.write('gone'); sys.exit(7)"],
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
    )
    process.wait(timeout=60)
    return process


def _write_events(path: Path, events: list[dict]) -> None:
    path.write_text(
        "".join(f"{json.dumps(event, sort_keys=True)}\n" for event in events),
        encoding="utf-8",
    )


def _never(_event: dict) -> bool:
    return False


def _dead_producer_fails_fast_instead_of_timing_out() -> None:
    """A dead producer must surface its exit, not the 15-minute timeout."""
    with tempfile.TemporaryDirectory(
        prefix="codestory-producer-liveness-self-test-"
    ) as raw:
        directory = Path(raw)
        nonce = "self-test-nonce"
        # The producer already wrote one unrelated telemetry record and then
        # stopped, exactly like the preserved Windows evidence.
        _write_events(
            directory / f"{nonce}.events.jsonl",
            [
                {
                    "schema_version": 1,
                    "sequence": 0,
                    "action": "completed_tokens",
                    "status": "completed",
                }
            ],
        )
        process = _dead_process()
        started = time.monotonic()
        try:
            send_server_qualification_control(
                directory,
                nonce,
                sequence=1,
                action="snapshot",
                timeout=_UNREACHED_TIMEOUT_SECS,
                producer=ChildProcessProducer(
                    process,
                    "the self-test producer",
                    "answering the first control",
                ),
            )
        except ProofFailure as error:
            elapsed = time.monotonic() - started
            message = str(error)
            require(
                elapsed < _ATTRIBUTION_BUDGET_SECS,
                "a dead producer still burned the whole qualification budget"
                f" ({elapsed:.1f}s) instead of failing fast",
            )
            require(
                "the self-test producer" in message
                and "exited with code 7" in message
                and "stdout_sha256=" in message
                and "stderr_sha256=" in message,
                f"a dead producer omitted its attributable exit state: {message}",
            )
            require(
                "sequence=1 action=snapshot" in message,
                f"a dead producer's failure did not name what was awaited: {message}",
            )
            require(
                "1 parsed record" in message
                and "completed_tokens" in message,
                "a dead producer's failure did not report how far the log got:"
                f" {message}",
            )
        else:
            raise ProofFailure("a dead producer completed a qualification control")
        require(
            not (directory / f"{nonce}.command.json").exists(),
            "an attributed control failure left its command file behind",
        )


def _a_record_written_just_before_exit_still_completes() -> None:
    """Exit must not beat a record the producer already appended."""
    with tempfile.TemporaryDirectory(
        prefix="codestory-producer-liveness-self-test-"
    ) as raw:
        directory = Path(raw)
        nonce = "self-test-nonce"
        _write_events(
            directory / f"{nonce}.events.jsonl",
            [
                {
                    "schema_version": 1,
                    "sequence": 1,
                    "action": "snapshot",
                    "status": "completed",
                }
            ],
        )
        event = send_server_qualification_control(
            directory,
            nonce,
            sequence=1,
            action="snapshot",
            timeout=_UNREACHED_TIMEOUT_SECS,
            producer=ChildProcessProducer(
                _dead_process(),
                "the self-test producer",
                "answering the first control",
            ),
        )
        require(
            event.get("sequence") == 1 and event.get("action") == "snapshot",
            "an exited producer lost a record it had already written",
        )


def _timeout_names_the_wait_the_log_and_the_producer() -> None:
    with tempfile.TemporaryDirectory(
        prefix="codestory-producer-liveness-self-test-"
    ) as raw:
        path = Path(raw) / "events.jsonl"
        _write_events(
            path,
            [
                {
                    "schema_version": 1,
                    "sequence": 0,
                    "action": "completed_tokens",
                    "status": "completed",
                }
            ],
        )
        try:
            wait_for_jsonl_event(
                path,
                _never,
                timeout=1,
                awaited="the self-test record",
                producer=UnobservedProducer(
                    "the self-test producer",
                    "doing self-test work",
                    "has not identified itself in this run",
                ),
            )
        except ProofFailure as error:
            message = str(error)
            require(
                "the self-test record" in message
                and "the self-test producer" in message
                and "doing self-test work" in message
                and "has not identified itself in this run" in message
                and "1 parsed record" in message
                and "completed_tokens" in message,
                "a qualification timeout did not name what was awaited, how far"
                f" the log got, and what the producer was doing: {message}",
            )
        else:
            raise ProofFailure("an unreachable qualification wait completed")


def _log_state_reports_every_shape_the_evidence_can_take() -> None:
    with tempfile.TemporaryDirectory(
        prefix="codestory-producer-liveness-self-test-"
    ) as raw:
        root = Path(raw)
        missing = root / "missing.jsonl"
        require(
            jsonl_log_state(missing) == "was never created",
            "a missing qualification log was not reported as missing",
        )
        empty = root / "empty.jsonl"
        empty.write_bytes(b"")
        require(
            "holds 0 bytes and no parsed record" in jsonl_log_state(empty),
            "an empty qualification log was not reported as empty",
        )
        torn = root / "torn.jsonl"
        torn.write_text('{"sequence":1,"action":"snapshot"}\n{"seq', encoding="utf-8")
        torn_state = jsonl_log_state(torn)
        require(
            "1 parsed record(s) out of 2 line(s)" in torn_state
            and "its last line is unterminated" in torn_state,
            f"a torn qualification log lost its partial-line evidence: {torn_state}",
        )


def _native_process_liveness_is_pinned_to_the_exact_process() -> None:
    live = NativeProcessProducer(
        os.getpid(),
        process_start_identity(os.getpid()),
        "the self-test host",
        "running this self-test",
    )
    require(
        not live.exited() and "is still running as pid" in live.state(),
        "a live pinned process was reported as gone",
    )
    recycled = NativeProcessProducer(
        os.getpid(),
        "linux:0" if not sys.platform.startswith("win") else "windows:0",
        "the self-test host",
        "running this self-test",
    )
    require(
        recycled.exited() and "replaced the pinned" in recycled.termination(),
        "a pid whose start identity changed was accepted as the pinned process",
    )
    process = _dead_process()
    process.communicate()
    gone = NativeProcessProducer(
        process.pid,
        process_start_identity(os.getpid()),
        "the self-test child",
        "running this self-test",
    )
    require(
        gone.exited() and "exited" in gone.termination(),
        "an exited process was not reported as exited",
    )


def _an_unreaped_process_is_never_reported_as_running() -> None:
    """A process that exited but was not reaped has exited, and must say so.

    On Linux `/proc/<pid>/stat` stays readable with an unchanged start time for
    a zombie, so a probe built on start identity alone reports a dead producer
    as "is still running as pid N" until its parent reaps it. That window is
    short, but the sentence it prints is false, and a proof that prints false
    liveness is worse than one that prints none.

    The stat parse is exercised from every host; the live zombie is only
    reachable where zombies exist.
    """
    for state, terminated in (
        ("Z", True),
        ("X", True),
        ("x", True),
        ("R", False),
        ("S", False),
        ("D", False),
        ("T", False),
    ):
        stat = f"4242 (a name with spaces) {state} 1 4242 4242 " + " ".join(
            str(field) for field in range(60)
        )
        observed = linux_terminated_state(stat)
        require(
            (observed is not None) == terminated,
            f"process state {state!r} was classified wrong: {observed!r}",
        )
    # Unparsable input must answer "cannot tell", never "has exited".
    require(
        linux_terminated_state("not a stat line") is None
        and linux_terminated_state("") is None,
        "an unreadable process state was reported as a termination",
    )
    require(
        terminated_process_state(os.getpid()) is None,
        "this live self-test process was reported as terminated",
    )
    if sys.platform != "linux":
        return
    # A child killed and deliberately not reaped is a zombie: /proc still has
    # its unchanged start identity, so only the state check can see the exit.
    zombie = subprocess.Popen([sys.executable, "-c", "raise SystemExit(0)"])
    pinned = NativeProcessProducer(
        zombie.pid,
        process_start_identity(zombie.pid),
        "the self-test zombie",
        "already exited without being reaped",
    )
    try:
        deadline = time.monotonic() + 30
        while time.monotonic() < deadline:
            if terminated_process_state(zombie.pid) is not None:
                break
            time.sleep(0.01)
        require(
            pinned.exited() and "waiting to be reaped" in pinned.termination(),
            "an exited but unreaped producer was reported as still running:"
            f" {pinned.state()}",
        )
    finally:
        zombie.wait(timeout=60)


def _a_group_names_which_member_died() -> None:
    survivor = UnobservedProducer(
        "the self-test survivor",
        "still working",
        "cannot be observed",
    )
    casualty = ChildProcessProducer(
        _dead_process(),
        "the self-test casualty",
        "already gone",
    )
    group = ProducerGroup([survivor, casualty])
    exited = group.exited()
    termination = group.termination()
    require(
        exited
        and "the self-test casualty" in termination
        and "exited with code 7" in termination,
        "a producer group did not name the member that died",
    )
    require(
        "the self-test survivor" in group.describe()
        and "the self-test casualty" in group.describe(),
        "a producer group omitted a member from its description",
    )
    observational = ProducerGroup(
        [
            survivor,
            ObservationalProducer(
                ChildProcessProducer(
                    _dead_process(),
                    "the deliberately crashed member",
                    "crashed on purpose",
                ),
                "crashed on purpose",
            ),
        ]
    )
    require(
        not observational.exited(),
        "a deliberately crashed producer was still allowed to fail a wait",
    )
    require(
        "exited with code 7" in observational.describe(),
        "a deliberately crashed producer lost its state from the description",
    )
    try:
        ProducerGroup([])
    except ProofFailure:
        pass
    else:
        raise ProofFailure("a qualification wait accepted an empty producer group")


def _control_events_pin_the_server_that_answered() -> None:
    pinned = server_producer_from_control_event(
        {
            "snapshot": {
                "process": {
                    "pid": os.getpid(),
                    "process_start_id": process_start_identity(os.getpid()),
                }
            }
        },
        "answering self-test controls",
    )
    require(
        isinstance(pinned, NativeProcessProducer)
        and pinned.pid == os.getpid()
        and not pinned.exited(),
        "a control event with an exact process identity was not pinned",
    )
    for event in (
        {},
        {"snapshot": {}},
        {"snapshot": {"process": {"pid": 0, "process_start_id": "linux:1"}}},
        {"snapshot": {"process": {"pid": 1, "process_start_id": ""}}},
        {"snapshot": {"process": {"pid": True, "process_start_id": "linux:1"}}},
    ):
        producer = server_producer_from_control_event(event, "answering controls")
        require(
            isinstance(producer, UnobservedProducer) and not producer.exited(),
            f"an identity-free control event produced a pinned server: {event!r}",
        )


def _every_qualification_wait_carries_a_producer() -> None:
    """Structural guard: the omission this lane fixed cannot come back.

    ``wait_for_jsonl_event`` and ``send_server_qualification_control`` both take
    ``producer`` as a keyword argument without a default, so an omission is a
    ``TypeError``. This walks the harness anyway, because a future caller could
    forward ``None`` or reintroduce a default and reproduce the exact defect.
    """
    package = REPOSITORY_ROOT / ".github" / "scripts" / "packaged_agent_proof"
    guarded = {"wait_for_jsonl_event", "send_server_qualification_control"}
    inspected = 0
    for source in sorted(package.glob("*.py")):
        tree = ast.parse(source.read_text(encoding="utf-8"), filename=str(source))
        for node in ast.walk(tree):
            if not isinstance(node, ast.Call):
                continue
            name = (
                node.func.id
                if isinstance(node.func, ast.Name)
                else getattr(node.func, "attr", None)
            )
            if name not in guarded:
                continue
            inspected += 1
            keyword = next(
                (
                    entry
                    for entry in node.keywords
                    if entry.arg == "producer"
                ),
                None,
            )
            require(
                keyword is not None,
                f"{source.name}:{node.lineno} calls {name} without a producer"
                " liveness argument, so a dead producer would degrade into an"
                " uninformative timeout",
            )
            require(
                not (
                    isinstance(keyword.value, ast.Constant)
                    and keyword.value.value is None
                ),
                f"{source.name}:{node.lineno} passes producer=None to {name}",
            )
        for node in ast.walk(tree):
            if isinstance(node, ast.FunctionDef) and node.name in guarded:
                defaults = node.args.kw_defaults
                index = [argument.arg for argument in node.args.kwonlyargs]
                require(
                    "producer" in index
                    and defaults[index.index("producer")] is None,
                    f"{node.name} gave its producer argument a default, which"
                    " lets a caller silently omit it again",
                )
    require(
        inspected >= 4,
        f"the qualification wait audit inspected only {inspected} call sites",
    )


def run_producer_liveness_self_tests() -> None:
    _dead_producer_fails_fast_instead_of_timing_out()
    _a_record_written_just_before_exit_still_completes()
    _timeout_names_the_wait_the_log_and_the_producer()
    _log_state_reports_every_shape_the_evidence_can_take()
    _native_process_liveness_is_pinned_to_the_exact_process()
    _an_unreaped_process_is_never_reported_as_running()
    _a_group_names_which_member_died()
    _control_events_pin_the_server_that_answered()
    _every_qualification_wait_carries_a_producer()
