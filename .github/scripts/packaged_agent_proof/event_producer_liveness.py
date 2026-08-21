"""Liveness attribution for the processes that append qualification events.

A wait that can only report "the record never arrived" cannot tell a slow
producer from one that exited, crashed, or never started, so every producer
failure collapses into the same uninformative timeout. Every qualification
wait in this harness therefore carries an explicit producer: what is expected
to append the record, what it was doing, and how its liveness can be observed.

A producer proven gone fails its wait immediately with an attributable
termination state. A producer whose liveness genuinely cannot be observed says
so in its own words, so even a timeout names the gap instead of hiding it.
"""

from __future__ import annotations

import hashlib
import subprocess

from .foundation import ProofFailure
from .process_identity import process_start_identity, terminated_process_state


def _stream_sha256(stream: str | bytes | None) -> str:
    if stream is None:
        payload = b""
    elif isinstance(stream, str):
        payload = stream.encode("utf-8")
    else:
        payload = stream
    return hashlib.sha256(payload).hexdigest()


class EventProducer:
    """One process expected to append the awaited qualification records."""

    def __init__(self, label: str, activity: str) -> None:
        self.label = label
        self.activity = activity

    def exited(self) -> bool:
        """Whether the producer is provably gone.

        Cheap and side-effect free: a wait calls this every poll, and must be
        able to re-read the log before deciding that an exit lost the race with
        a record the producer already wrote.
        """
        return False

    def termination(self) -> str:
        """Attributable termination state; only called once ``exited()`` holds."""
        return "is no longer running"

    def state(self) -> str:
        """Current state, for the timeout path."""
        return "cannot be observed from this proof host"

    def describe(self) -> str:
        return f"{self.label} ({self.activity}) {self.state()}"


class UnobservedProducer(EventProducer):
    """A producer this host holds no handle on, with the reason stated."""

    def __init__(self, label: str, activity: str, reason: str) -> None:
        super().__init__(label, activity)
        self._reason = reason

    def state(self) -> str:
        return self._reason


class ChildProcessProducer(EventProducer):
    """A producer this harness spawned and still holds a handle on."""

    def __init__(
        self,
        process: subprocess.Popen,
        label: str,
        activity: str,
    ) -> None:
        super().__init__(label, activity)
        self._process = process

    def exited(self) -> bool:
        return self._process.poll() is not None

    def termination(self) -> str:
        stdout, stderr = self._process.communicate()
        return (
            f"exited with code {self._process.returncode}"
            f" stdout_sha256={_stream_sha256(stdout)}"
            f" stderr_sha256={_stream_sha256(stderr)}"
        )

    def state(self) -> str:
        code = self._process.poll()
        if code is None:
            return f"is still running as pid {self._process.pid}"
        return f"exited with code {code}"


class NativeProcessProducer(EventProducer):
    """A producer pinned by exact native process identity, not by pid alone."""

    def __init__(
        self,
        pid: int,
        process_start_id: str,
        label: str,
        activity: str,
    ) -> None:
        super().__init__(label, activity)
        self.pid = pid
        self.process_start_id = process_start_id

    def _gone(self) -> str | None:
        """A description of how the exact pinned process ended, or None."""
        # Start identity alone cannot reliably classify an exited-but-unreaped
        # Unix process: Linux keeps the same /proc start time, while macOS may
        # stop returning a complete proc_pidinfo record before the PID is
        # reaped. Ask for a terminated state first so this never reports a dead
        # producer as running.
        terminated = terminated_process_state(self.pid)
        if terminated is not None:
            return f"exited: pid {self.pid} {terminated}"
        try:
            observed = process_start_identity(self.pid)
        except (FileNotFoundError, ProcessLookupError):
            return f"exited: pid {self.pid} no longer exists"
        except ProofFailure as error:
            return (
                f"exited: pid {self.pid} is no longer observable as a running"
                f" process ({error})"
            )
        except OSError as error:
            return f"exited: pid {self.pid} could not be inspected ({error})"
        if observed != self.process_start_id:
            return (
                f"exited: pid {self.pid} now carries start identity {observed},"
                f" which replaced the pinned {self.process_start_id}"
            )
        return None

    def exited(self) -> bool:
        return self._gone() is not None

    def termination(self) -> str:
        return self._gone() or "is still running"

    def state(self) -> str:
        return self._gone() or (
            f"is still running as pid {self.pid}"
            f" (start identity {self.process_start_id})"
        )


class ObservationalProducer(EventProducer):
    """Report another producer's state without ever failing a wait on it.

    Used for a process whose exit is expected by the scenario -- a server the
    proof itself crashed -- where the state is still worth naming in a failure.
    """

    def __init__(self, producer: EventProducer, activity: str) -> None:
        super().__init__(producer.label, activity)
        self._producer = producer

    def state(self) -> str:
        return self._producer.state()


class ProducerGroup(EventProducer):
    """Every process whose loss would explain a missing record."""

    def __init__(self, producers: list[EventProducer]) -> None:
        if not producers:
            raise ProofFailure("a qualification wait needs at least one producer")
        super().__init__(
            " and ".join(producer.label for producer in producers),
            " and ".join(producer.activity for producer in producers),
        )
        self.producers = list(producers)

    def _first_exited(self) -> EventProducer | None:
        return next(
            (producer for producer in self.producers if producer.exited()),
            None,
        )

    def exited(self) -> bool:
        return self._first_exited() is not None

    def termination(self) -> str:
        producer = self._first_exited()
        if producer is None:
            return "is still running"
        return f"{producer.label} {producer.termination()}"

    def state(self) -> str:
        return "; ".join(producer.describe() for producer in self.producers)

    def describe(self) -> str:
        return self.state()
