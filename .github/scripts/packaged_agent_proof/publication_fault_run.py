"""One publication fault injection and recovery run."""

from __future__ import annotations

import hashlib
import secrets
import subprocess
from pathlib import Path

from .contract_primitives import write_private_json
from .event_producer_liveness import (
    ChildProcessProducer,
    ObservationalProducer,
    ProducerGroup,
    UnobservedProducer,
)
from .foundation import require
from .publication_fault_setup import _restore_fixture
from .publication_fault_types import (
    PublicationCandidate,
    PublicationCommands,
    PublicationFaultRun,
    PublicationFixture,
)
from .publication_protocol import (
    SERVER_PRODUCER_LABEL,
    control_timeout_secs,
    ensure_resident_qualification_server,
    read_jsonl,
    run_publication_replacement_worker,
    send_control_to_resident_server,
    send_server_qualification_control,
    server_producer_from_control_event,
    wait_for_jsonl_event,
)

CANDIDATE_PRODUCER_LABEL = "the publication candidate process"


def _start_fault_candidate(
    env: dict[str, str],
    private_root: Path,
    nonce: str,
    fixture: PublicationFixture,
    commands: PublicationCommands,
) -> PublicationCandidate:
    correlation_id = secrets.token_hex(16)
    nonce_sha256 = hashlib.sha256(nonce.encode("ascii")).hexdigest()
    pause_path = private_root / f"publication-pause-{nonce_sha256}.json"
    resume_path = private_root / f"publication-resume-{correlation_id}.json"
    event_path = private_root / f"publication-events-{correlation_id}.jsonl"
    write_private_json(
        pause_path,
        {
            "schema_version": 1,
            "nonce_sha256": nonce_sha256,
            "correlation_id": correlation_id,
            "action": "pause_before_manifest_commit",
        },
    )
    fixture.source_file.write_text(
        fixture.baseline_source
        + "// publication qualification candidate source change\n",
        encoding="utf-8",
    )
    fixture.lexical_file.write_text(
        "# Publication qualification candidate\n",
        encoding="utf-8",
    )
    process = subprocess.Popen(
        commands.retrieval_index,
        cwd=fixture.project,
        env=env,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
    )
    return PublicationCandidate(
        correlation_id,
        nonce_sha256,
        pause_path,
        resume_path,
        event_path,
        process,
    )


def _run_fault(
    cli: Path,
    env: dict[str, str],
    private_root: Path,
    nonce: str,
    fixture: PublicationFixture,
    commands: PublicationCommands,
    *,
    executable_sha256: str,
    timeout: int,
) -> PublicationFaultRun:
    # The baseline publication embeds once, at its very start, and then spends
    # the rest of its time in packaged CLI invocations that do no embedding at
    # all. By the time this run's first control is written, that gap has already
    # exceeded the server's frozen 60s idle budget on a slow host -- 703s on the
    # Windows calibration cell against a 3.9s gap on Linux -- so the server that
    # served the baseline has correctly exited, no process is left to consume
    # the command file, and writing one starts nothing. The wait was therefore
    # deadlocked, not slow, and burned the whole proof budget every time.
    #
    # The harness owes this residency, not the product: idle exit releases the
    # resident model, is a frozen contract value, and is itself the subject of
    # the `true_idle_exit` measurement, so extending it to survive a harness
    # phase would corrupt the constant it measures. One admitted query
    # establishes residency by construction on every platform, and pins the
    # exact server that will answer.
    control_timeout = control_timeout_secs(timeout)
    snapshot_before = send_control_to_resident_server(
        private_root,
        nonce,
        sequence=1,
        action="snapshot",
        timeout=control_timeout,
        # Each attempt writes its own worker request and output. The worker
        # refuses to overwrite an existing output, so a shared label would make
        # the tolerated respawn unrunnable -- the replacement would die before
        # it started, and the run would report that instead of the lost server.
        establish=lambda attempt: ensure_resident_qualification_server(
            cli,
            env,
            fixture.project,
            private_root,
            nonce,
            executable_sha256=executable_sha256,
            timeout=timeout,
            activity="answering the first control of this publication fault run",
            label=f"fault-residency-{attempt}",
        ),
    )
    resident_server = server_producer_from_control_event(
        snapshot_before,
        "answering the publication fault controls",
    )
    candidate = _start_fault_candidate(
        env,
        private_root,
        nonce,
        fixture,
        commands,
    )
    candidate_producer = ChildProcessProducer(
        candidate.process,
        CANDIDATE_PRODUCER_LABEL,
        "indexing the paused candidate publication",
    )
    stdout = ""
    stderr = ""
    try:
        wait_for_jsonl_event(
            candidate.event_path,
            lambda event: (
                event.get("action") == "pause_before_manifest_commit"
                and event.get("status") == "waiting_for_resume"
            ),
            timeout=timeout,
            awaited="the publication hook pause_before_manifest_commit event",
            producer=candidate_producer,
        )
        # This control's residency comes from the candidate itself: it embedded
        # the mutated fixture on its way to the manifest fence, so the server's
        # idle window restarted during that work. Re-establishing residency here
        # would put an extra admitted request inside the fault window this step
        # exists to observe, so it is deliberately not done.
        #
        # That is a judgement, not a bound. Nothing here measures the interval
        # between the candidate's last embed and the fence, and on a host where
        # one packaged CLI invocation costs the better part of a minute it is
        # not comfortably inside the 60s budget. The residual risk is accepted
        # rather than hidden: the control budget turns it into a fast, named
        # timeout instead of the half hour of anonymous silence that made this
        # class of failure so expensive to diagnose.
        #
        # The pinned predecessor is reported as state, never as cause: sequence
        # 2's whole purpose is to end that server, so its exit proves nothing
        # about this wait. Only the candidate process -- which nothing replaces
        # -- can fail it.
        crash_event = send_server_qualification_control(
            private_root,
            nonce,
            sequence=2,
            action="crash_server",
            timeout=control_timeout,
            producer=ProducerGroup(
                [
                    ObservationalProducer(
                        resident_server,
                        "answering the crash control, unless it exited first and"
                        " a replacement server answered instead",
                    ),
                    candidate_producer,
                ]
            ),
        )
        run_publication_replacement_worker(
            cli,
            env,
            fixture.project,
            private_root,
            nonce,
            crash_event=crash_event,
            candidate_producer=candidate_producer,
            executable_sha256=executable_sha256,
            timeout=timeout,
        )
        # Sequence 2 crashed the pinned server on purpose, so its exit proves
        # nothing here: a replacement server must start to answer this control.
        # The crashed predecessor stays in the report as state, never as cause.
        snapshot_after = send_server_qualification_control(
            private_root,
            nonce,
            sequence=3,
            action="snapshot",
            timeout=control_timeout,
            producer=ProducerGroup(
                [
                    UnobservedProducer(
                        f"the replacement for {SERVER_PRODUCER_LABEL}",
                        "answering the post-crash snapshot control",
                        "has not identified itself: the sequence-2 server was"
                        " crashed on purpose, so a replacement must start and"
                        " answer before this control can complete",
                    ),
                    ObservationalProducer(
                        resident_server,
                        "the server this run crashed at sequence 2",
                    ),
                    candidate_producer,
                ]
            ),
        )
        write_private_json(
            candidate.resume_path,
            {
                "schema_version": 1,
                "nonce_sha256": candidate.nonce_sha256,
                "correlation_id": candidate.correlation_id,
                "action": "resume_manifest_commit",
            },
        )
        stdout, stderr = candidate.process.communicate(timeout=timeout)
    except BaseException:
        if candidate.process.poll() is None:
            candidate.process.kill()
            stdout, stderr = candidate.process.communicate()
        raise
    finally:
        _restore_fixture(fixture)
    require(
        candidate.process.returncode is not None and candidate.process.returncode != 0,
        "publication candidate did not fail after losing its server lease",
    )
    events = read_jsonl(candidate.event_path)
    require(len(events) == 4, "publication hook did not emit its exact four events")
    return PublicationFaultRun(
        candidate.correlation_id,
        candidate.pause_path,
        candidate.resume_path,
        snapshot_before,
        snapshot_after,
        candidate.process.returncode,
        stdout,
        stderr,
        events,
    )
