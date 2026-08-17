"""One authenticated qualification directory per resident embedding server.

``CODESTORY_EMBED_QUALIFICATION_DIR`` is read once per process. A server binds
it at start and polls that one directory for the rest of its life, so rebinding
the variable mid-run moves only the writers: a server an earlier phase left
resident keeps polling the directory it was started with, every control written
under the new directory is read by nobody, and the wait that follows can only
end as an anonymous timeout.

Proof run 30600248269 ended exactly there. Publication-fault production left a
replacement server polling ``qualification-suite``; the measurement producer
then rebound the variable to ``qualification-suite/artifacts`` without replacing
that server; ``reset_owner`` observed the live server, wrote ``crash_server``
under the child directory, and spent the rest of the Windows job on
``embedding_qualification_control_event_timeout:crash_server`` waiting for an
event from a process that was never going to read the command.

Every binding goes through this module so the two rules that make that
impossible are stated once: a directory is bound before anything is resident on
it, and a rebind replaces the resident server *before* moving the writers and
then proves, by exact process identity, that the server answering the new
directory is not the one it was supposed to replace. The proof is what keeps
the fix honest. A restored rebind fails here, by name, for the seconds one
admitted query costs, instead of by timeout after the whole proof budget.
"""

from __future__ import annotations

from dataclasses import dataclass
from pathlib import Path

from .event_producer_liveness import EventProducer, NativeProcessProducer
from .foundation import ProofFailure, require
from .publication_protocol import (
    PUBLICATION_CRASH_EXIT_ALLOWANCE_SECS,
    control_timeout_secs,
    ensure_resident_qualification_server,
    read_jsonl,
    server_producer_from_control_event,
    send_control_to_resident_server,
    wait_for_accepted_crash_exit,
)

QUALIFICATION_DIRECTORY_ENV = "CODESTORY_EMBED_QUALIFICATION_DIR"
# The one code a stranded control producer may fail with. It names the seam --
# which directory the server is bound to versus which directory the controls
# are written under -- so the failure is diagnosable from the message alone,
# which `embedding_qualification_control_event_timeout:crash_server` never was.
QUALIFICATION_DIRECTORY_MISMATCH = "embedding_qualification_control_directory_mismatch"


@dataclass(frozen=True)
class ResidentQualificationServer:
    """The exact server process answering controls under one directory."""

    directory: str
    producer: EventProducer
    pid: int
    process_start_id: str

    @property
    def identity(self) -> tuple[int, str]:
        return (self.pid, self.process_start_id)

    def describe(self) -> str:
        return (
            f"pid {self.pid} (start identity {self.process_start_id})"
            f" bound to {self.directory}"
        )


def bound_qualification_directory(env: dict[str, str]) -> str | None:
    return env.get(QUALIFICATION_DIRECTORY_ENV)


def bind_qualification_directory(
    env: dict[str, str],
    directory: Path | str,
    *,
    server_cleanup_control: dict | None = None,
) -> str:
    """Bind every later control, event, and worker file to one directory.

    Only safe while nothing is resident on the directory being replaced, which
    is why the initial bind calls this and a mid-run move calls
    ``rebind_qualification_directory`` instead.
    """
    resolved = Path(directory).resolve()
    require(
        resolved.is_dir() and not resolved.is_symlink(),
        f"qualification directory is not a private directory: {resolved}",
    )
    value = str(resolved)
    env[QUALIFICATION_DIRECTORY_ENV] = value
    if server_cleanup_control is not None:
        server_cleanup_control["qualification_directory"] = value
    return value


def _next_control_sequence(directory: Path, nonce: str) -> int:
    """One past every sequence the server in this directory has answered.

    The server refuses a sequence it has already recorded, and it reloads the
    highest one from this log at start, so a replacement server rejects a reused
    number exactly as its predecessor would. Deriving it from the log keeps this
    caller independent of how many controls an earlier phase happened to issue.
    """
    highest = max(
        (
            event["sequence"]
            for event in read_jsonl(directory / f"{nonce}.events.jsonl")
            if isinstance(event.get("sequence"), int)
            and not isinstance(event.get("sequence"), bool)
        ),
        default=0,
    )
    return highest + 1


def _pinned_server(
    producer: EventProducer,
    directory: str,
    phase: str,
) -> ResidentQualificationServer:
    if not isinstance(producer, NativeProcessProducer):
        raise ProofFailure(
            f"{QUALIFICATION_DIRECTORY_MISMATCH}: the server made resident on"
            f" {directory} {phase} did not report an exact process identity"
            f" ({producer.describe()}), so this rebind cannot prove which"
            " directory the process answering qualification controls is bound to"
        )
    return ResidentQualificationServer(
        directory,
        producer,
        producer.pid,
        producer.process_start_id,
    )


def rebind_qualification_directory(
    directory: Path,
    *,
    cli: Path,
    env: dict[str, str],
    project: Path,
    nonce: str,
    executable_sha256: str,
    timeout: int,
    server_cleanup_control: dict,
    label: str,
    crash_exit_allowance_secs: float = PUBLICATION_CRASH_EXIT_ALLOWANCE_SECS,
) -> dict:
    """Move every control producer to ``directory``, replacing the server first.

    The replacement is unconditional. Absence of a resident server cannot be
    observed -- a probe that finds none is indistinguishable from one that ran
    before the server this phase is about to inherit finished starting -- so the
    only fail-closed order is to make one resident, end it, and only then move
    the writers.
    """
    previous = bound_qualification_directory(env)
    require(
        previous is not None,
        "a qualification directory rebind needs a bound directory to move from",
    )
    target = str(Path(directory).resolve())
    if previous == target:
        return {"rebound": False, "directory": target, "replaced": None}
    previous_directory = Path(previous)
    established: list[EventProducer] = []

    def establish(attempt: int) -> EventProducer:
        # Each attempt names its own worker evidence: the worker refuses to
        # overwrite an output, so a shared label would make the tolerated
        # respawn unrunnable.
        producer = ensure_resident_qualification_server(
            cli,
            env,
            project,
            previous_directory,
            nonce,
            executable_sha256=executable_sha256,
            timeout=timeout,
            activity=(
                "answering the control that ends it before the qualification"
                f" directory moves to {target}"
            ),
            label=f"{label}-{attempt}",
        )
        established.append(producer)
        return producer

    crash_event = send_control_to_resident_server(
        previous_directory,
        nonce,
        sequence=_next_control_sequence(previous_directory, nonce),
        action="crash_server",
        timeout=control_timeout_secs(timeout),
        establish=establish,
    )
    replaced = _pinned_server(
        server_producer_from_control_event(
            crash_event,
            "exiting after accepting the directory-rebind crash control",
        ),
        previous,
        "to be replaced",
    )
    wait_for_accepted_crash_exit(
        replaced.producer,
        None,
        allowance_secs=crash_exit_allowance_secs,
    )
    replaced_exited = True
    bind_qualification_directory(
        env,
        directory,
        server_cleanup_control=server_cleanup_control,
    )
    successor = _pinned_server(
        ensure_resident_qualification_server(
            cli,
            env,
            project,
            Path(target),
            nonce,
            executable_sha256=executable_sha256,
            timeout=timeout,
            activity=(
                "answering every qualification control written under the"
                " rebound directory"
            ),
            label=f"{label}-successor",
        ),
        target,
        "after the rebind",
    )
    if successor.identity == replaced.identity:
        raise ProofFailure(
            f"{QUALIFICATION_DIRECTORY_MISMATCH}: {replaced.describe()} survived"
            f" the crash control that was supposed to replace it and is still the"
            f" server answering for {previous}, while every later control and"
            f" event is written under {target}. Nothing polls the new directory,"
            " so the next control has no reader and would fail as an"
            " unattributed timeout instead of naming this"
        )
    return {
        "rebound": True,
        "directory": target,
        "previous_directory": previous,
        "replaced": replaced,
        "replaced_exited": replaced_exited,
        "successor": successor,
    }
