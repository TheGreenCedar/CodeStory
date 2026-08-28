"""Publication qualification protocol operations."""

from __future__ import annotations

import hashlib
import json
import os
import re
import sys
import time
from pathlib import Path

from .contract_primitives import (
    canonical_sha256,
    require_nonempty_string,
    require_opaque_identifier,
    require_positive_int,
    require_sha256,
    write_private_json,
)
from .event_producer_liveness import (
    EventProducer,
    NativeProcessProducer,
    UnobservedProducer,
)
from .foundation import (
    EMBEDDING_QUALIFICATION_WORKER_SCHEMA_VERSION,
    SERVER_QUALIFICATION_CONTROL_TIMEOUT_SECS,
    ProofFailure,
    require,
)
from .process_identity import ExactProcessExitWaiter
from .subprocess_control import json_command, run, validate_v3_search_projection

SERVER_PRODUCER_LABEL = "the per-user embedding qualification server"
_WORKER_LABEL = re.compile(r"^[a-z][a-z0-9-]{0,31}$")
_COMMAND_UNLINK_TIMEOUT_SECS = 2.0
_CRASH_EXIT_POLL_SECS = 0.05
# A harness allowance for an already-accepted abort to become an observable
# native process exit. This is deliberately independent of the proof timeout,
# product idle/teardown constants, and calibration. It stays below the
# qualification lane's 90-second native-microprobe ceiling so a process that
# never exits fails by name before any replacement query can race it.
PUBLICATION_CRASH_EXIT_ALLOWANCE_SECS = 75.0
PUBLICATION_CRASH_EXIT_TIMEOUT = (
    "embedding_qualification_crashed_server_exit_timeout"
)
PUBLICATION_CRASH_IDENTITY_MISSING = (
    "embedding_qualification_crashed_server_identity_missing"
)
PUBLICATION_CRASH_NOT_ACCEPTED = (
    "embedding_qualification_crash_server_not_accepted"
)


def _unlink_control_command(path: Path) -> None:
    """Remove a consumed command after a bounded Windows sharing retry."""
    deadline = time.monotonic() + _COMMAND_UNLINK_TIMEOUT_SECS
    while True:
        try:
            path.unlink(missing_ok=True)
            return
        except PermissionError:
            if time.monotonic() >= deadline:
                raise
            time.sleep(0.01)


def control_timeout_secs(timeout: int) -> int:
    """The budget one qualification control may spend, never the proof budget.

    A control is answered by a resident server in milliseconds, so the only
    thing a longer wait can buy is a slower report of a server that is not
    there. The proof budget is still honoured as a ceiling: a self-test or a
    caller running on a shorter budget never gets a longer one from here.
    """
    return max(1, min(timeout, SERVER_QUALIFICATION_CONTROL_TIMEOUT_SECS))


def read_jsonl(path: Path) -> list[dict]:
    if not path.is_file() or path.is_symlink():
        return []
    events = []
    for line in path.read_text(encoding="utf-8").splitlines():
        if not line.strip():
            continue
        try:
            event = json.loads(line)
        except json.JSONDecodeError:
            continue
        if isinstance(event, dict):
            events.append(event)
    return events


def jsonl_log_state(path: Path) -> str:
    """How far the awaited log actually got, in one attributable sentence.

    A wait that fails must never leave open whether the log was missing, empty,
    torn, or simply short of the awaited record: those have different causes and
    only the log itself can tell them apart.
    """
    if path.is_symlink():
        return "is a symlink and was never a trustworthy qualification log"
    try:
        raw = path.read_bytes()
    except FileNotFoundError:
        return "was never created"
    except OSError as error:
        return f"could not be read: {error}"
    lines = [line for line in raw.split(b"\n") if line.strip()]
    records = read_jsonl(path)
    trailer = "" if not raw or raw.endswith(b"\n") else "; its last line is unterminated"
    if not records:
        return f"holds {len(raw)} bytes and no parsed record{trailer}"
    sequences = [
        event["sequence"]
        for event in records
        if isinstance(event.get("sequence"), int)
    ]
    last = records[-1]
    actions = sorted({str(event.get("action")) for event in records})
    return (
        f"holds {len(raw)} bytes and {len(records)} parsed record(s)"
        f" out of {len(lines)} line(s); actions {', '.join(actions)};"
        f" highest sequence {max(sequences) if sequences else 'none'};"
        f" last record sequence={last.get('sequence')!r}"
        f" action={last.get('action')!r} status={last.get('status')!r}{trailer}"
    )


def wait_for_jsonl_event(
    path: Path,
    predicate,
    *,
    timeout: int,
    awaited: str,
    producer: EventProducer,
) -> dict:
    """Wait for one qualification record, attributing every way it can fail.

    ``producer`` has no default on purpose. A wait without a liveness argument
    turns any producer failure -- exit, crash, or a server that idled out before
    the control was written -- into the same anonymous timeout, which is the
    defect this signature exists to prevent. Callers with nothing to observe
    pass an explicit ``UnobservedProducer`` that states why.
    """
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        for event in read_jsonl(path):
            if predicate(event):
                return event
        if producer.exited():
            # The producer may have appended the awaited record and exited
            # between the read above and this probe, so the log decides last.
            for event in read_jsonl(path):
                if predicate(event):
                    return event
            raise ProofFailure(
                f"{producer.label} ({producer.activity}) {producer.termination()}"
                f" before {awaited} was written:"
                f" qualification event file {path.name} {jsonl_log_state(path)}"
            )
        time.sleep(0.01)
    raise ProofFailure(
        f"timed out after {timeout}s waiting for {awaited}:"
        f" qualification event file {path.name} {jsonl_log_state(path)};"
        f" {producer.describe()}"
    )


def server_producer_from_control_event(event: dict, activity: str) -> EventProducer:
    """Pin the exact server process that answered one control, when it named it."""
    return server_producer_from_snapshot(event.get("snapshot"), activity)


def server_producer_from_snapshot(snapshot: object, activity: str) -> EventProducer:
    """Pin the exact server a snapshot names, from a control event or a worker.

    A control event and an embedding worker's ``final_snapshot`` carry the same
    ``EmbeddingServerSnapshot``, so both pin the same way. A snapshot without an
    exact process identity degrades to a producer that states the gap rather
    than to a pin that could name the wrong process.
    """
    process = snapshot.get("process") if isinstance(snapshot, dict) else None
    pid = process.get("pid") if isinstance(process, dict) else None
    process_start_id = (
        process.get("process_start_id") if isinstance(process, dict) else None
    )
    if (
        not isinstance(pid, int)
        or isinstance(pid, bool)
        or pid <= 0
        or not isinstance(process_start_id, str)
        or not process_start_id
    ):
        return UnobservedProducer(
            SERVER_PRODUCER_LABEL,
            activity,
            "did not report an exact process identity in the snapshot that named"
            " it, so its liveness cannot be pinned",
        )
    return NativeProcessProducer(
        pid,
        process_start_id,
        SERVER_PRODUCER_LABEL,
        activity,
    )


def send_server_qualification_control(
    directory: Path,
    nonce: str,
    *,
    sequence: int,
    action: str,
    timeout: int,
    producer: EventProducer,
) -> dict:
    nonce_sha256 = hashlib.sha256(nonce.encode("ascii")).hexdigest()
    command_path = directory / f"{nonce}.command.json"
    require(
        not command_path.exists(), "stale embedding qualification command is present"
    )
    write_private_json(
        command_path,
        {
            "schema_version": 1,
            "sequence": sequence,
            "nonce_sha256": nonce_sha256,
            "action": action,
            "parameters": {"class": None},
        },
    )
    try:
        event_path = directory / f"{nonce}.events.jsonl"
        event = wait_for_jsonl_event(
            event_path,
            lambda candidate: (
                candidate.get("sequence") == sequence
                and candidate.get("action") == action
            ),
            timeout=timeout,
            awaited=(
                f"embedding qualification control sequence={sequence}"
                f" action={action}"
            ),
            producer=producer,
        )
        require(
            event.get("status") in {"completed", "accepted"},
            f"embedding qualification control {action} failed",
        )
        return event
    finally:
        _unlink_control_command(command_path)


def server_observation_from_control_event(event: dict, phase: str) -> dict:
    snapshot = event.get("snapshot")
    require(
        isinstance(snapshot, dict), f"{phase} control event omitted its server snapshot"
    )
    process = snapshot.get("process")
    engine = snapshot.get("engine")
    require(
        isinstance(process, dict), f"{phase} server snapshot omitted process identity"
    )
    require(
        isinstance(engine, dict),
        f"{phase} server snapshot omitted resident engine identity",
    )
    process_start = require_nonempty_string(
        process.get("process_start_id"), f"{phase} process start"
    )
    return {
        "phase": phase,
        "server_instance_id": require_opaque_identifier(
            process.get("server_instance_id"), f"{phase} server instance"
        ),
        "process_start_id": hashlib.sha256(process_start.encode("utf-8")).hexdigest(),
        "load_generation": require_positive_int(
            engine.get("load_generation"), f"{phase} load generation"
        ),
    }


def publication_identity_from_status(status: dict) -> str:
    require(status.get("retrieval_mode") == "full", "qualification status is not full")
    contract = status.get("manifest_contract")
    manifest = status.get("manifest")
    require(
        isinstance(contract, dict),
        "qualification status omitted its manifest contract",
    )
    require(
        isinstance(manifest, dict),
        "qualification status omitted its published manifest",
    )
    generation = require_nonempty_string(
        contract.get("generation"), "qualification manifest contract generation"
    )
    input_hash = require_sha256(
        contract.get("input_hash"), "qualification manifest contract input hash"
    )
    project_id = require_opaque_identifier(
        contract.get("project_id"), "qualification manifest contract project"
    )
    schema_version = require_positive_int(
        contract.get("schema_version"), "qualification manifest contract schema"
    )
    graph_hash = require_sha256(
        contract.get("graph_hash"), "qualification manifest contract graph hash"
    )
    require(
        manifest.get("project_id") == project_id
        and manifest.get("sidecar_generation") == generation
        and manifest.get("sidecar_input_hash") == input_hash
        and manifest.get("sidecar_schema_version") == schema_version
        and manifest.get("graph_artifact_hash") == graph_hash,
        "qualification manifest report disagrees with its manifest contract",
    )
    return canonical_sha256(
        {
            "project_id": project_id,
            "generation": generation,
            "input_hash": input_hash,
            "schema_version": schema_version,
            "graph_hash": graph_hash,
            "lexical_version": require_nonempty_string(
                manifest.get("lexical_version"),
                "qualification manifest lexical version",
            ),
            "semantic_generation": require_nonempty_string(
                manifest.get("semantic_generation"),
                "qualification manifest semantic generation",
            ),
            "scip_revision": require_nonempty_string(
                manifest.get("scip_revision"),
                "qualification manifest SCIP revision",
            ),
        }
    )


def _search_evidence_contains_expected(
    project: Path, evidence: dict, expected: str
) -> bool:
    require(
        isinstance(evidence, dict),
        f"qualification search emitted non-object evidence: {evidence!r}",
    )
    raw_path = evidence.get("path")
    require(
        isinstance(raw_path, str) and bool(raw_path),
        f"qualification search evidence omitted its source path: {evidence!r}",
    )
    project_root = project.resolve(strict=True)
    candidate = Path(raw_path)
    source_path = (candidate if candidate.is_absolute() else project_root / candidate).resolve(
        strict=True
    )
    require(
        source_path.is_relative_to(project_root),
        f"qualification search evidence escaped the project root: {evidence!r}",
    )
    start_line = evidence.get("start_line")
    end_line = evidence.get("end_line")
    if not (
        isinstance(start_line, int)
        and not isinstance(start_line, bool)
        and isinstance(end_line, int)
        and not isinstance(end_line, bool)
        and 1 <= start_line <= end_line
    ):
        return False
    lines = source_path.read_text(encoding="utf-8").splitlines()
    require(
        end_line <= len(lines),
        f"qualification search evidence source range is out of bounds: {evidence!r}",
    )
    return expected in "\n".join(lines[start_line - 1 : end_line])


def rank_v3_search_evidence(
    project: Path, payload: dict, query: str, expected: str
) -> int | None:
    projection = dict(payload)
    metadata = projection.pop("_meta", None)
    require(
        isinstance(metadata, dict),
        f"qualification CLI search omitted its transport metadata: {payload!r}",
    )
    evidence, retrieval = validate_v3_search_projection(
        projection,
        query,
        "qualification CLI search",
    )
    require(
        retrieval.get("state") == "full",
        f"qualification CLI search was not full retrieval: {projection!r}",
    )
    position = next(
        (
            index
            for index, item in enumerate(evidence[:10])
            if _search_evidence_contains_expected(project, item, expected)
        ),
        None,
    )
    return None if position is None else position + 1


def run_quality_search(
    cli: Path,
    env: dict[str, str],
    project: Path,
    run_id: str,
    query: str,
    expected: str,
    *,
    timeout: int,
) -> tuple[int | None, str]:
    result, payload = json_command(
        [
            str(cli),
            "search",
            "--project",
            str(project),
            "--query",
            query,
            "--limit",
            "10",
            "--repo-text",
            "off",
            "--refresh",
            "none",
            "--profile",
            "agent",
            "--run-id",
            run_id,
            "--format",
            "json",
        ],
        env=env,
        cwd=project,
        timeout=timeout,
    )
    rank = rank_v3_search_evidence(project, payload, query, expected)
    output_sha256 = hashlib.sha256(result["stdout"].encode("utf-8")).hexdigest()
    return rank, output_sha256


def run_embedding_qualification_query_worker(
    cli: Path,
    env: dict[str, str],
    project: Path,
    private_root: Path,
    nonce: str,
    *,
    executable_sha256: str,
    timeout: int,
    label: str,
) -> dict:
    """Complete exactly one admitted embedding query, and return its result.

    This is the harness's only way to make the product do real embedding work on
    demand, and therefore its only way to establish or renew server residency:
    an admitted request is what restarts the server's idle window. Observing a
    server does not, and must not -- ``true_idle_respawn`` asserts that the idle
    epoch never moves under observation, so an observer that extended the
    observed lifetime would corrupt the very constant being measured.

    ``label`` separates one caller's request and output files from another's, so
    two callers in the same run never read each other's evidence -- and so that
    two invocations can both run. The worker refuses to overwrite an existing
    output (`embedding_qualification_output_exists`), which makes a reused label
    a one-shot: the second invocation cannot complete, whatever it was for. That
    is checked here rather than left to the CLI so a reused label is reported as
    the harness defect it is, at the caller that reused it, instead of as an
    opaque non-zero exit from a command that never got to run.
    """
    require(
        isinstance(label, str) and _WORKER_LABEL.fullmatch(label) is not None,
        f"embedding qualification worker label is unusable: {label!r}",
    )
    executable_sha256 = require_sha256(
        executable_sha256,
        f"publication {label} worker executable sha256",
    )
    request_path = private_root / f"publication-{label}-worker-request.json"
    output_path = private_root / f"publication-{label}-worker-output.json"
    require(
        not output_path.exists() and not output_path.is_symlink(),
        f"embedding qualification worker label {label!r} was already used in this"
        f" run: {output_path.name} exists, and the worker refuses to overwrite an"
        " output, so this invocation could never complete. Give each invocation"
        " its own label instead of retrying into the previous one's evidence",
    )
    write_private_json(
        request_path,
        {
            "schema_version": EMBEDDING_QUALIFICATION_WORKER_SCHEMA_VERSION,
            "nonce_sha256": hashlib.sha256(nonce.encode("ascii")).hexdigest(),
            "executable_sha256": executable_sha256,
            "project": str(project.resolve()),
            "operation": "query",
            "parameters": {
                "query_count": 1,
                "bulk_count": 0,
                "documents_per_bulk": 0,
                "input_bytes": 64,
                "hold_ms": 0,
            },
        },
    )
    run(
        [
            str(cli),
            "internal-embedding-qualification-worker",
            "--request",
            str(request_path),
            "--output",
            str(output_path),
        ],
        env=env,
        cwd=project,
        timeout=timeout,
    )
    require(
        output_path.is_file() and not output_path.is_symlink(),
        f"publication {label} worker omitted its output",
    )
    try:
        output = json.loads(output_path.read_text(encoding="utf-8"))
    except json.JSONDecodeError as exc:
        raise ProofFailure(
            f"publication {label} worker output is not valid JSON: {exc}"
        ) from exc
    require(
        isinstance(output, dict)
        and output.get("schema_version")
        == EMBEDDING_QUALIFICATION_WORKER_SCHEMA_VERSION
        and output.get("executable_sha256") == executable_sha256
        and output.get("error") is None,
        f"publication {label} worker failed",
    )
    result = output.get("result")
    operations = result.get("operations") if isinstance(result, dict) else None
    # The inner EmbeddingQualificationResult contract versions independently
    # of the outer worker wire contract checked above.
    require(
        isinstance(result, dict)
        and result.get("schema_version") == 1
        and result.get("scenario") == "query"
        and isinstance(operations, list)
        and len(operations) == 1
        and operations[0].get("status") == "ok"
        and operations[0].get("error_code") is None,
        f"publication {label} worker did not complete its query",
    )
    return result


def run_publication_replacement_worker(
    cli: Path,
    env: dict[str, str],
    project: Path,
    private_root: Path,
    nonce: str,
    *,
    crash_event: dict,
    candidate_producer: EventProducer,
    executable_sha256: str,
    timeout: int,
    crash_exit_allowance_secs: float = PUBLICATION_CRASH_EXIT_ALLOWANCE_SECS,
) -> None:
    """Start the server that replaces the one sequence 2 crashed on purpose.

    The crash event is the durable acceptance receipt and names the exact
    process that accepted it. A new admitted query may start only after that
    pinned PID plus process-start identity is proven gone. Otherwise the query
    can race the still-live authority and report an owner-unresponsive product
    failure that the harness created. The paused publication candidate is the
    other irreplaceable producer, so its exit aborts this wait immediately.

    Once the predecessor is gone, use the same one-query residency primitive as
    the rest of the harness, under the name the post-crash step calls it by and
    with its own immutable request/output files.
    """
    require(
        isinstance(crash_event, dict)
        and crash_event.get("action") == "crash_server"
        and crash_event.get("status") == "accepted",
        f"{PUBLICATION_CRASH_NOT_ACCEPTED}: replacement requires the durable"
        " accepted crash_server event",
    )
    predecessor = server_producer_from_control_event(
        crash_event,
        "exiting after durably accepting the publication crash control",
    )
    wait_for_accepted_crash_exit(
        predecessor,
        candidate_producer,
        allowance_secs=crash_exit_allowance_secs,
    )
    run_embedding_qualification_query_worker(
        cli,
        env,
        project,
        private_root,
        nonce,
        executable_sha256=executable_sha256,
        timeout=timeout,
        label="replacement",
    )


def wait_for_accepted_crash_exit(
    predecessor: EventProducer,
    candidate: EventProducer | None,
    *,
    allowance_secs: float = PUBLICATION_CRASH_EXIT_ALLOWANCE_SECS,
) -> dict:
    """Prove the accepted-crash predecessor gone before any fresh query.

    ``ExactProcessExitWaiter`` is the identity fence: on Windows it holds a
    synchronization handle to the exact process object; on Unix it requires
    either absence, a terminated state, or PID reuse with a different start
    identity. Inspection failures are not exit evidence. A non-native producer
    cannot support the claim and therefore fails closed rather than treating an
    unobservable server as absent.
    """
    if (
        not isinstance(predecessor, NativeProcessProducer)
        or not isinstance(predecessor.pid, int)
        or isinstance(predecessor.pid, bool)
        or predecessor.pid <= 0
        or not isinstance(predecessor.process_start_id, str)
        or not predecessor.process_start_id
    ):
        raise ProofFailure(
            f"{PUBLICATION_CRASH_IDENTITY_MISSING}: the accepted crash_server"
            " event did not pin a positive PID and nonempty process-start"
            f" identity ({predecessor.describe()})"
        )
    require(
        isinstance(allowance_secs, (int, float))
        and not isinstance(allowance_secs, bool)
        and 0 < allowance_secs < 90,
        "publication crash-exit allowance must be positive and below 90 seconds",
    )
    started = time.monotonic()
    deadline = started + allowance_secs
    target_os = (
        "windows"
        if os.name == "nt"
        else ("macos" if sys.platform == "darwin" else "linux")
    )
    waiter = ExactProcessExitWaiter(
        predecessor.pid,
        predecessor.process_start_id,
        target_os,
        allow_already_exited=True,
    )
    try:
        first_probe = True
        while True:
            # The candidate is irreplaceable and must remain paused throughout
            # the crash fence. Check it first so simultaneous process loss is
            # never misreported as permission to start a replacement.
            if candidate is not None and candidate.exited():
                raise ProofFailure(
                    f"{candidate.label} ({candidate.activity})"
                    f" {candidate.termination()} while waiting for crashed server"
                    f" pid {predecessor.pid} (start identity"
                    f" {predecessor.process_start_id}) to exit; no replacement"
                    " worker was invoked"
                )
            now = time.monotonic()
            # Always admit one immediate probe. The native waiter may need a
            # little construction time to open and inspect its held Windows
            # handle, and an already-gone predecessor must not become a timeout
            # solely because that setup crossed the allowance. Every later
            # poll is gated before probing, so an overslept/late exit cannot be
            # accepted after the named bound.
            if not first_probe and now >= deadline:
                if candidate is not None and candidate.exited():
                    raise ProofFailure(
                        f"{candidate.label} ({candidate.activity})"
                        f" {candidate.termination()} while waiting for crashed"
                        f" server pid {predecessor.pid} (start identity"
                        f" {predecessor.process_start_id}) to exit; no replacement"
                        " worker was invoked"
                    )
                raise ProofFailure(
                    f"{PUBLICATION_CRASH_EXIT_TIMEOUT}: pid {predecessor.pid}"
                    f" (start identity {predecessor.process_start_id}) remained"
                    f" live for {int((now - started) * 1000)}ms after accepting"
                    " crash_server; no replacement worker was invoked"
                )
            exited = waiter.exited()
            if candidate is not None and candidate.exited():
                raise ProofFailure(
                    f"{candidate.label} ({candidate.activity})"
                    f" {candidate.termination()} while waiting for crashed"
                    f" server pid {predecessor.pid} (start identity"
                    f" {predecessor.process_start_id}) to exit; no replacement"
                    " worker was invoked"
                )
            observed_at = time.monotonic()
            if exited and (first_probe or observed_at < deadline):
                return {
                    "pid": predecessor.pid,
                    "process_start_id": predecessor.process_start_id,
                    "waited_ms": int((observed_at - started) * 1000),
                }
            if observed_at >= deadline:
                raise ProofFailure(
                    f"{PUBLICATION_CRASH_EXIT_TIMEOUT}: pid {predecessor.pid}"
                    f" (start identity {predecessor.process_start_id}) remained"
                    f" live for {int((observed_at - started) * 1000)}ms after"
                    " accepting crash_server; no replacement worker was invoked"
                )
            first_probe = False
            now = observed_at
            time.sleep(min(_CRASH_EXIT_POLL_SECS, deadline - now))
    finally:
        waiter.close()


def ensure_resident_qualification_server(
    cli: Path,
    env: dict[str, str],
    project: Path,
    private_root: Path,
    nonce: str,
    *,
    executable_sha256: str,
    timeout: int,
    activity: str,
    label: str,
) -> EventProducer:
    """Make a server resident for the control about to be issued, and pin it.

    Residency is not something a control can ask for. The server exits on
    ``true_idle`` after its frozen 60s budget, a control poll deliberately does
    not restart that window, and writing a command file starts no process, so a
    control issued into a gap longer than the budget waits for a process that
    already exited and never returns. That is correct product behaviour: idle
    exit releases the resident model and is itself a measured constant. The
    harness, not the product, owes the residency it depends on.

    One admitted query establishes it, and the worker's ``final_snapshot`` then
    names the exact server that will answer, so an exit during the control is
    attributable instead of anonymous.
    """
    result = run_embedding_qualification_query_worker(
        cli,
        env,
        project,
        private_root,
        nonce,
        executable_sha256=executable_sha256,
        timeout=timeout,
        label=label,
    )
    return server_producer_from_snapshot(result.get("final_snapshot"), activity)


def send_control_to_resident_server(
    directory: Path,
    nonce: str,
    *,
    sequence: int,
    action: str,
    timeout: int,
    establish,
) -> dict:
    """Issue one control against a server this harness proved resident first.

    Establishing residency removes the deadlock; tolerating one respawn keeps
    the removal honest. A server may still exit between the establishing query
    and the control -- that is legitimate, measured product behaviour, and
    failing the run on it would reject correct product conduct. So a pinned exit
    costs exactly one re-establishment and one re-issued control, never an
    unbounded retry loop and never a silent pass: a second loss fails closed and
    names both servers.

    ``establish`` is called with the attempt number, 1 and then 2, and returns
    the pinned producer for the server it made resident, so the wait it guards
    fails in seconds with a named process instead of spending its budget on
    anonymous silence. The attempt is not decoration: an establishing step that
    writes evidence has to write the second attempt's evidence somewhere the
    first attempt's does not already sit, or the tolerance this function exists
    to provide cannot run at all. Every failure on the way is attributed to the
    loss that started it, including a replacement that could not be established.
    """
    producer = establish(1)
    try:
        return send_server_qualification_control(
            directory,
            nonce,
            sequence=sequence,
            action=action,
            timeout=timeout,
            producer=producer,
        )
    except ProofFailure as first_failure:
        if not producer.exited():
            # The established server is still running, so this failure is the
            # control's own answer -- a rejected action, a stale command file, a
            # torn log -- and re-issuing it would only hide it.
            raise
        lost = f"{producer.label} {producer.termination()}"
        first = str(first_failure)
    try:
        replacement = establish(2)
    except ProofFailure as replacement_failure:
        raise ProofFailure(
            f"embedding qualification control sequence={sequence} action={action}"
            f" lost its resident server: {lost} ({first}); no replacement server"
            f" could be established to answer it ({replacement_failure})"
        ) from replacement_failure
    try:
        return send_server_qualification_control(
            directory,
            nonce,
            sequence=sequence,
            action=action,
            timeout=timeout,
            producer=replacement,
        )
    except ProofFailure as second_failure:
        raise ProofFailure(
            f"embedding qualification control sequence={sequence} action={action}"
            f" lost its resident server twice: {lost} ({first}); its replacement"
            f" then failed the same control ({second_failure})"
        ) from second_failure
