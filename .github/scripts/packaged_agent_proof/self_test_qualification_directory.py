"""Regression for the qualification directory a resident server is bound to.

Proof run 30600248269 died as
``embedding_qualification_control_event_timeout:crash_server`` on the Windows
cell after real Vulkan execution had already passed. The cause was not the
product: publication-fault production left a replacement server polling
``qualification-suite``, the measurement producer rebound
``CODESTORY_EMBED_QUALIFICATION_DIR`` to ``qualification-suite/artifacts``
without replacing that server, and every later control was written into a
directory nothing was polling.

The tests here reproduce that parent/child mismatch deterministically, without
building or running the packaged product: a stub CLI that hands back whichever
server is already resident -- exactly the singleton behaviour that makes the
defect possible -- and a stub server bound for life to the directory it was
started with. In ``exits`` mode the crash control replaces it and the rebind
succeeds. In ``survives`` mode the parent-directory server outlives the crash,
which is the restored defect, and the rebind must fail by name in seconds
rather than spend a proof budget on an unattributed timeout.

Everything here is portable to a Windows-native runner: no POSIX-only
primitive, no signals, no fork, and a total cost measured in seconds.
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
from types import SimpleNamespace
from unittest.mock import patch

from . import qualification_producer_runner
from .foundation import (
    EMBEDDING_QUALIFICATION_WORKER_SCHEMA_VERSION,
    REPOSITORY_ROOT,
    ProofFailure,
    require,
)
from .publication_protocol import PUBLICATION_CRASH_EXIT_TIMEOUT, read_jsonl
from .qualification_directory_binding import (
    QUALIFICATION_DIRECTORY_ENV,
    QUALIFICATION_DIRECTORY_MISMATCH,
    bind_qualification_directory,
    bound_qualification_directory,
    rebind_qualification_directory,
)

_NONCE = "self-test-rebind-nonce"
_LABEL = "rebind-self-test"
_EXECUTABLE_SHA256 = "a" * 64
# Long enough that nothing here is bounded by it, short enough that a defect
# that ignores the named check still cannot hang a `--self-test` run.
_PROOF_BUDGET_SECS = 60
# The rebind pays one settling window plus two stub worker invocations. A
# failure that takes longer than this is the anonymous timeout this lane exists
# to replace, whatever its message says.
_NAMED_FAILURE_BUDGET_SECS = 20
# The publication fault run owns sequences 1..3 on the parent directory, and a
# server refuses a sequence it has already recorded, so the replacement control
# has to continue that log rather than restart it.
_PUBLICATION_FAULT_SEQUENCES = (1, 2, 3)

# A server bound for life to the directory it was started with, like the
# product: it polls that one directory for commands and appends its events
# there. `survives` is the restored defect -- it answers the crash control and
# keeps running, still polling the parent directory.
_SERVER_STUB = '''\
import json
import os
import sys
import time
from pathlib import Path

directory = Path(sys.argv[1])
nonce = sys.argv[2]
state = Path(sys.argv[3])
mode = sys.argv[4]
command_path = directory / (nonce + ".command.json")
events_path = directory / (nonce + ".events.jsonl")
answered = set()
deadline = time.monotonic() + 120
while time.monotonic() < deadline:
    try:
        command = json.loads(command_path.read_text(encoding="utf-8"))
    except (OSError, ValueError):
        time.sleep(0.01)
        continue
    sequence = command.get("sequence")
    action = command.get("action")
    if sequence in answered:
        time.sleep(0.01)
        continue
    answered.add(sequence)
    snapshot = None
    if action == "crash_server":
        recorded = json.loads(state.read_text(encoding="utf-8"))
        snapshot = {
            "process": {
                "pid": os.getpid(),
                "process_start_id": recorded["process_start_id"],
            }
        }
    with open(events_path, "a", encoding="utf-8") as handle:
        handle.write(
            json.dumps(
                {
                    "schema_version": 1,
                    "sequence": sequence,
                    "action": action,
                    "status": "accepted" if action == "crash_server" else "completed",
                    "snapshot": snapshot,
                },
                sort_keys=True,
            )
            + "\\n"
        )
    if action == "crash_server" and mode == "exits":
        break
    time.sleep(0.01)
try:
    if json.loads(state.read_text(encoding="utf-8")).get("pid") == os.getpid():
        state.unlink()
except (OSError, ValueError):
    pass
'''

# A stand-in for `codestory-cli internal-embedding-qualification-worker` with
# the two clauses that decide this lane: the worker's request and output must
# be direct children of the bound qualification directory (worker.rs
# `validate_direct_child`), and one admitted query is served by whichever
# server is already resident for this user -- whatever directory that server
# was bound to. That second clause is the whole defect: the worker cannot move
# a running server to a new directory, so a rebind that does not replace it
# strands every control the harness writes afterwards.
_WORKER_STUB = '''\
import json
import os
import subprocess
import sys
from pathlib import Path

sys.path.insert(0, os.environ["SELF_TEST_SCRIPTS_ROOT"])
from packaged_agent_proof.process_identity import (
    process_start_identity,
    terminated_process_state,
)

argv = sys.argv[1:]
if not argv or argv[0] != "internal-embedding-qualification-worker":
    sys.stderr.write("unexpected subcommand: " + " ".join(argv) + "\\n")
    raise SystemExit(2)
request_path = Path(argv[argv.index("--request") + 1])
output_path = Path(argv[argv.index("--output") + 1])
directory = Path(os.environ["CODESTORY_EMBED_QUALIFICATION_DIR"])
if request_path.parent != directory or output_path.parent != directory:
    sys.stderr.write("embedding_qualification_worker_directory_mismatch\\n")
    raise SystemExit(4)
if output_path.exists():
    sys.stderr.write("embedding_qualification_output_exists\\n")
    raise SystemExit(1)
request = json.loads(request_path.read_text(encoding="utf-8"))
state = Path(os.environ["SELF_TEST_SERVER_STATE"])


def resident():
    try:
        recorded = json.loads(state.read_text(encoding="utf-8"))
    except (OSError, ValueError):
        return None
    pid = recorded.get("pid")
    if not isinstance(pid, int):
        return None
    if terminated_process_state(pid) is not None:
        return None
    try:
        observed = process_start_identity(pid)
    except BaseException:
        return None
    return recorded if observed == recorded.get("process_start_id") else None


server = resident()
if server is None:
    process = subprocess.Popen(
        [
            sys.executable,
            os.environ["SELF_TEST_SERVER_SCRIPT"],
            str(directory),
            os.environ["CODESTORY_EMBED_QUALIFICATION_NONCE"],
            str(state),
            os.environ["SELF_TEST_SERVER_MODE"],
        ],
        stdin=subprocess.DEVNULL,
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
    )
    server = {
        "pid": process.pid,
        "process_start_id": process_start_identity(process.pid),
        "server_instance_id": "self-test-server-" + str(process.pid),
        "directory": str(directory),
    }
    state.write_text(json.dumps(server, sort_keys=True), encoding="utf-8")
output_path.write_text(
    json.dumps(
        {
            "schema_version": int(os.environ["SELF_TEST_WORKER_SCHEMA_VERSION"]),
            "executable_sha256": request["executable_sha256"],
            "error": None,
            "result": {
                "schema_version": 1,
                "scenario": "query",
                "operations": [{"status": "ok", "error_code": None}],
                "final_snapshot": {
                    "process": {
                        "pid": server["pid"],
                        "process_start_id": server["process_start_id"],
                        "server_instance_id": server["server_instance_id"],
                    }
                },
            },
        }
    ),
    encoding="utf-8",
)
'''


def _stub_cli(directory: Path, script: Path) -> Path:
    """The stub as something the harness invokes exactly as it invokes a CLI."""
    if os.name == "nt":
        cli = directory / "qualification-stub.cmd"
        cli.write_text(
            f'@echo off\r\n"{sys.executable}" "{script}" %*\r\n',
            encoding="utf-8",
        )
        return cli
    cli = directory / "qualification-stub"
    # `#!/bin/sh` stays inside every platform's shebang length limit; the
    # interpreter path, which does not, lives in the body instead.
    cli.write_text(
        f'#!/bin/sh\nexec "{sys.executable}" "{script}" "$@"\n',
        encoding="utf-8",
    )
    cli.chmod(0o700)
    return cli


class _Fixture:
    """A parent qualification directory with a child artifact directory."""

    def __init__(self, root: Path, mode: str) -> None:
        self.root = root
        self.private_root = root / "qualification-suite"
        self.artifact_root = self.private_root / "artifacts"
        self.private_root.mkdir(mode=0o700)
        self.artifact_root.mkdir(mode=0o700)
        self.project = root / "project"
        self.project.mkdir()
        binaries = root / "bin"
        binaries.mkdir()
        server_script = binaries / "server-stub.py"
        server_script.write_text(_SERVER_STUB, encoding="utf-8")
        worker_script = binaries / "worker-stub.py"
        worker_script.write_text(_WORKER_STUB, encoding="utf-8")
        self.cli = _stub_cli(binaries, worker_script)
        self.state = root / "server-state.json"
        self.control: dict = {}
        self.env = dict(os.environ)
        self.env.update(
            {
                "SELF_TEST_SCRIPTS_ROOT": str(REPOSITORY_ROOT / ".github" / "scripts"),
                "SELF_TEST_SERVER_SCRIPT": str(server_script),
                "SELF_TEST_SERVER_STATE": str(self.state),
                "SELF_TEST_SERVER_MODE": mode,
                "SELF_TEST_WORKER_SCHEMA_VERSION": str(
                    EMBEDDING_QUALIFICATION_WORKER_SCHEMA_VERSION
                ),
                "CODESTORY_EMBED_QUALIFICATION_NONCE": _NONCE,
            }
        )
        bind_qualification_directory(
            self.env,
            self.private_root,
            server_cleanup_control=self.control,
        )
        self._seed_publication_fault_events()

    def _seed_publication_fault_events(self) -> None:
        """The three controls the publication fault run already answered here."""
        with open(
            self.private_root / f"{_NONCE}.events.jsonl",
            "a",
            encoding="utf-8",
        ) as handle:
            for sequence in _PUBLICATION_FAULT_SEQUENCES:
                handle.write(
                    json.dumps(
                        {
                            "schema_version": 1,
                            "sequence": sequence,
                            "action": "snapshot",
                            "status": "completed",
                        },
                        sort_keys=True,
                    )
                    + "\n"
                )

    def rebind(self) -> dict:
        return rebind_qualification_directory(
            self.artifact_root,
            cli=self.cli,
            env=self.env,
            project=self.project,
            nonce=_NONCE,
            executable_sha256=_EXECUTABLE_SHA256,
            timeout=_PROOF_BUDGET_SECS,
            server_cleanup_control=self.control,
            label=_LABEL,
            crash_exit_allowance_secs=0.25,
        )

    def parent_events(self) -> list[dict]:
        return read_jsonl(self.private_root / f"{_NONCE}.events.jsonl")

    def stop_stub_servers(self) -> None:
        try:
            recorded = json.loads(self.state.read_text(encoding="utf-8"))
        except (OSError, ValueError):
            return
        pid = recorded.get("pid")
        if not isinstance(pid, int):
            return
        try:
            if os.name == "nt":
                subprocess.run(
                    ["taskkill", "/PID", str(pid), "/F"],
                    check=False,
                    capture_output=True,
                    timeout=30,
                )
            else:
                os.kill(pid, 9)
        except (OSError, subprocess.SubprocessError):
            pass


def _fixture(mode: str):
    class _Scope:
        def __enter__(self) -> _Fixture:
            self._temporary = tempfile.TemporaryDirectory(
                prefix="codestory-qualification-directory-self-test-"
            )
            self.fixture = _Fixture(Path(self._temporary.__enter__()), mode)
            return self.fixture

        def __exit__(self, *error: object) -> None:
            self.fixture.stop_stub_servers()
            self._temporary.__exit__(None, None, None)

    return _Scope()


def _a_replaced_server_lets_every_producer_move_together() -> None:
    """The fix: replace the resident server, then move every writer."""
    with _fixture("exits") as fixture:
        result = fixture.rebind()
        target = str(fixture.artifact_root.resolve())
        previous = str(fixture.private_root.resolve())
        require(
            result["rebound"] is True
            and result["directory"] == target
            and result["previous_directory"] == previous,
            "the qualification directory rebind did not report the move it made",
        )
        require(
            bound_qualification_directory(fixture.env) == target
            and fixture.control["qualification_directory"] == target,
            "a rebind left a control or event producer on the previous"
            " qualification directory",
        )
        require(
            result["successor"].identity != result["replaced"].identity,
            "the server answering the rebound directory is the one the rebind"
            " was supposed to replace",
        )
        events = fixture.parent_events()
        crash = [event for event in events if event.get("action") == "crash_server"]
        require(
            len(crash) == 1
            and crash[0]["sequence"] == max(_PUBLICATION_FAULT_SEQUENCES) + 1
            and crash[0]["status"] == "accepted",
            "the replacement control did not continue the parent directory's"
            f" own control sequence: {events}",
        )
        require(
            not (fixture.private_root / f"{_NONCE}.command.json").exists(),
            "the replacement control left its command file behind",
        )
        require(
            (
                fixture.private_root / f"publication-{_LABEL}-1-worker-output.json"
            ).is_file(),
            "the replaced server was established from outside the directory it"
            " was bound to",
        )
        require(
            (
                fixture.artifact_root
                / f"publication-{_LABEL}-successor-worker-output.json"
            ).is_file(),
            "the successor server was established from outside the rebound"
            " directory",
        )


def _a_surviving_parent_directory_server_fails_by_name() -> None:
    """The restored defect: exact exit must precede every successor query.

    This is proof run 30600248269 in miniature. The parent-directory server
    outlives the crash control, so it is still the server every later control
    would be answered by -- while those controls are written into the child
    directory it has never polled.
    """
    with _fixture("survives") as fixture:
        started = time.monotonic()
        try:
            fixture.rebind()
        except ProofFailure as error:
            elapsed = time.monotonic() - started
            message = str(error)
            require(
                elapsed < _NAMED_FAILURE_BUDGET_SECS,
                "a surviving parent-directory server still cost the proof"
                f" budget ({elapsed:.1f}s) instead of failing by name",
            )
            require(
                message.startswith(PUBLICATION_CRASH_EXIT_TIMEOUT + ":")
                and "pid " in message
                and "start identity" in message,
                f"the surviving exact predecessor was not named: {message}",
            )
            require(
                bound_qualification_directory(fixture.env)
                == str(fixture.private_root.resolve())
                and not (
                    fixture.artifact_root
                    / f"publication-{_LABEL}-successor-worker-output.json"
                ).exists(),
                "a surviving predecessor let the directory move or successor"
                " worker run before exact exit",
            )
            require(
                "timed out after" not in message,
                f"the directory mismatch degraded into a generic timeout: {message}",
            )
        else:
            raise ProofFailure(
                "a rebind that left a server polling the parent directory was"
                " accepted, so every later control would be written where"
                " nothing reads it"
            )


def _rebinding_to_the_bound_directory_replaces_nothing() -> None:
    """A no-op move must not crash the server the run is about to use."""
    with _fixture("exits") as fixture:
        bind_qualification_directory(
            fixture.env,
            fixture.artifact_root,
            server_cleanup_control=fixture.control,
        )
        result = fixture.rebind()
        require(
            result["rebound"] is False
            and result["directory"] == str(fixture.artifact_root.resolve()),
            "a rebind to the already-bound directory reported a move",
        )
        require(
            not fixture.state.exists(),
            "a no-op rebind started and crashed a server for nothing",
        )


# The two measurements the scenario runner drives through `reset_owner`. If
# they and the inflight `server_crash` scenario were ever split across two
# producer invocations, a rebind could sit between them again and only the
# second half would reach the server.
_RESET_DRIVEN_METRICS = {"cold_first_vector", "spawn_convergence"}


def _the_measurement_reset_and_server_crash_share_one_producer_sequence() -> None:
    """One rebind, then one producer sequence carrying reset and crash alike."""
    with tempfile.TemporaryDirectory(
        prefix="codestory-qualification-sequence-self-test-"
    ) as raw:
        artifact_root = Path(raw)
        context = SimpleNamespace(
            args=SimpleNamespace(
                expected_backend="metal",
                qualification_matrix_cell="protected_macos_arm64_metal",
                proof_tier="protected_hardware",
                engine_policy="accelerated",
                offline=True,
                timeout_secs=_PROOF_BUDGET_SECS,
            ),
            runtime={"identity": {"embedding_backend": "metal"}},
            manifest={
                "source": {"commit": "0" * 40, "tree": "0" * 40, "tracked_dirty": False},
                "asset_target": "macos-arm64",
            },
            package={"executable_sha256": _EXECUTABLE_SHA256},
            contracts={"protocol_sha256": "b" * 64},
            projects=("/self-test/project-a", "/self-test/project-b"),
            measurement_contract={
                "measurement_protocol": {
                    "required_metrics": sorted(
                        _RESET_DRIVEN_METRICS | {"true_idle_exit"}
                    )
                }
            },
            artifact_root=artifact_root,
            nonce=_NONCE,
            nonce_sha256="c" * 64,
            qualification_env={},
            server_cleanup_control={},
            qualification_cli=Path("/self-test/cli"),
            qualification_driver=Path("/self-test/driver"),
            root=artifact_root,
        )
        ordered: list[str] = []
        requests: list[dict] = []

        def record_rebind(*_args: object, **_keywords: object) -> dict:
            ordered.append("rebind")
            return {"rebound": True}

        def record_request(path: Path, payload: dict) -> None:
            ordered.append("request")
            requests.append(payload)

        def record_run(*_args: object, **_keywords: object) -> dict:
            ordered.append("run")
            return {}

        with (
            patch.object(
                qualification_producer_runner,
                "selected_qualification_matrix_cell",
                return_value={"cache_state": "reused", "residency_state": "resident"},
            ),
            patch.object(
                qualification_producer_runner,
                "rebind_qualification_directory",
                side_effect=record_rebind,
            ),
            patch.object(
                qualification_producer_runner,
                "write_private_json",
                side_effect=record_request,
            ),
            patch.object(
                qualification_producer_runner,
                "run",
                side_effect=record_run,
            ),
            patch.object(
                qualification_producer_runner,
                "_validated_qualification_output",
                return_value={},
            ),
        ):
            qualification_producer_runner.run_qualification_producer(context)
    require(
        ordered == ["rebind", "request", "run"],
        "the qualification producer no longer replaces its server, writes one"
        f" request, and runs one producer sequence in that order: {ordered}",
    )
    require(len(requests) == 1, "the qualification producer wrote more than one request")
    request = requests[0]
    require(
        "server_crash" in request["required_scenarios"]
        and _RESET_DRIVEN_METRICS <= set(request["required_metrics"]),
        "the inflight server_crash scenario and the reset-driven measurements no"
        " longer travel in the same producer sequence, so a directory rebind"
        " could once again sit between them",
    )


def _call_name(node: ast.Call) -> str | None:
    if isinstance(node.func, ast.Name):
        return node.func.id
    return node.func.attr if isinstance(node.func, ast.Attribute) else None


def _is_directory_key(node: ast.expr) -> bool:
    if isinstance(node, ast.Constant):
        return node.value == QUALIFICATION_DIRECTORY_ENV
    if isinstance(node, ast.Name):
        return node.id == "QUALIFICATION_DIRECTORY_ENV"
    return (
        isinstance(node, ast.Attribute)
        and node.attr == "QUALIFICATION_DIRECTORY_ENV"
    )


def _directory_environment_writes(tree: ast.AST) -> list[int]:
    lines = []
    for node in ast.walk(tree):
        if isinstance(node, ast.Assign):
            lines.extend(
                target.lineno
                for target in node.targets
                if isinstance(target, ast.Subscript) and _is_directory_key(target.slice)
            )
        elif isinstance(node, ast.Dict):
            lines.extend(
                key.lineno
                for key in node.keys
                if key is not None and _is_directory_key(key)
            )
    return sorted(lines)


# Modules allowed to write the qualification directory environment variable,
# each with the reason it is not the defect this lane closes: every one of them
# binds an environment for a phase that has no resident server yet. A phase
# that binds while a server is already resident has to call
# `rebind_qualification_directory`, which replaces that server first.
_DIRECTORY_BINDERS = {
    "qualification_directory_binding.py": "owns binding and server replacement",
    "installation_support.py": "binds the installed-runtime proof root first",
    "server_cleanup.py": "rebinds the recorded directory for the cleanup client",
    "self_test_contract_scope.py": "builds a self-test fixture environment",
}


def _only_the_binding_owner_moves_the_qualification_directory() -> None:
    """Structural guard: the rebind that stranded the controls cannot come back."""
    package = REPOSITORY_ROOT / ".github" / "scripts" / "packaged_agent_proof"
    inspected = 0
    for source in sorted(package.glob("*.py")):
        tree = ast.parse(source.read_text(encoding="utf-8"), filename=str(source))
        writes = _directory_environment_writes(tree)
        if not writes:
            continue
        inspected += len(writes)
        require(
            source.name in _DIRECTORY_BINDERS,
            f"{source.name}:{writes[0]} binds"
            f" {QUALIFICATION_DIRECTORY_ENV} directly. A server binds that"
            " variable once, at start, and polls that one directory for life,"
            " so a phase that moves it while a server is resident strands every"
            " control it writes afterwards. Call"
            " rebind_qualification_directory, which replaces the resident"
            " server before the writers move",
        )
    require(
        inspected >= len(_DIRECTORY_BINDERS),
        f"the qualification directory binding audit inspected only {inspected}"
        " bindings, so it can no longer see the phase that moves them",
    )


def _the_measurement_producer_replaces_its_server_before_rebinding() -> None:
    """The exact site the Windows qualification cell stranded its controls."""
    package = REPOSITORY_ROOT / ".github" / "scripts" / "packaged_agent_proof"
    source = package / "qualification_producer_runner.py"
    text = source.read_text(encoding="utf-8")
    require(
        QUALIFICATION_DIRECTORY_ENV not in text,
        "qualification_producer_runner.py names the qualification directory"
        " environment variable again: this producer runs after the"
        " publication-fault phase has left a server resident on the parent"
        " directory, so it may only move through rebind_qualification_directory",
    )
    tree = ast.parse(text, filename=str(source))
    producer = next(
        (
            node
            for node in ast.walk(tree)
            if isinstance(node, ast.FunctionDef)
            and node.name == "run_qualification_producer"
        ),
        None,
    )
    require(
        producer is not None,
        "qualification_producer_runner lost its run_qualification_producer owner",
    )
    ordered = sorted(
        (node.lineno, _call_name(node))
        for node in ast.walk(producer)
        if isinstance(node, ast.Call)
        and _call_name(node)
        in {"rebind_qualification_directory", "write_private_json", "run"}
    )
    require(
        bool(ordered) and ordered[0][1] == "rebind_qualification_directory",
        "qualification_producer_runner.py writes the scenario runner's request"
        " or starts it before replacing the server bound to the parent"
        " qualification directory, which is the ordering that produced"
        " embedding_qualification_control_event_timeout:crash_server",
    )


def run_qualification_directory_self_tests() -> None:
    _a_replaced_server_lets_every_producer_move_together()
    _a_surviving_parent_directory_server_fails_by_name()
    _rebinding_to_the_bound_directory_replaces_nothing()
    _the_measurement_reset_and_server_crash_share_one_producer_sequence()
    _only_the_binding_owner_moves_the_qualification_directory()
    _the_measurement_producer_replaces_its_server_before_rebinding()
