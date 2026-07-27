"""True-idle server lifecycle self-tests.

Two defects live at this boundary and only one of them is the product's.

The product exits on `true_idle` after its frozen 60s budget, and a control poll
deliberately does not restart that window: `true_idle_respawn` asserts the idle
epoch never moves under observation, and `true_idle_exit` measures the budget
itself. Both are correct and neither may be relaxed to make a proof pass.

The harness, meanwhile, used to issue the publication fault run's first control
into a gap it never bounded -- 3.9s on Linux, 703s on Windows -- so on a slow
host the server had correctly exited, no process was left to consume the command
file, and writing one starts nothing. The wait was deadlocked, not slow, and
spent the whole 1800s proof budget proving nothing. The tests below pin the
harness side: residency is established before a control, a control is bounded by
the budget that governs the server it waits on, and one legitimate respawn is
tolerated while a second loss fails closed.

That last property is the one a stubbed test cannot see. Residency is
established by running the embedding qualification worker, and the worker
refuses to overwrite an existing output, so two attempts that name their
evidence identically cannot both run -- the tolerance reads as implemented while
being unreachable. One test therefore drives the real establishing path through
a worker that keeps that refusal, rather than a stand-in that cannot fail on it.
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

from .event_producer_liveness import ChildProcessProducer, UnobservedProducer
from .foundation import (
    EMBEDDING_QUALIFICATION_WORKER_SCHEMA_VERSION,
    PER_USER_EMBEDDING_SERVER_IDLE_TIMEOUT_MS,
    REPOSITORY_ROOT,
    SERVER_CONSTANT_SET,
    SERVER_QUALIFICATION_CONTROL_TIMEOUT_SECS,
    ProofFailure,
    require,
)
from .publication_protocol import (
    SERVER_PRODUCER_LABEL,
    control_timeout_secs,
    ensure_resident_qualification_server,
    send_control_to_resident_server,
)
from .qualification_scenario_assertions import derive_scenario_assertions
from .self_test_full_stack_types import ServerIdentityFixture, TrueIdleFixture


def _true_idle_snapshots(
    first_snapshot: dict,
) -> tuple[dict, dict, list[dict]]:
    true_idle_before = json.loads(json.dumps(first_snapshot))
    true_idle_respawned = json.loads(json.dumps(first_snapshot))
    true_idle_respawned["process"]["server_instance_id"] = "respawned-server"
    true_idle_respawned["process"]["pid"] = 303
    true_idle_respawned["process"]["process_start_id"] = "process-start-303"
    true_idle_respawned["authority"]["lifetime_authority_id"] = "respawned-authority"
    true_idle_respawned["authority"]["listener_id"] = "respawned-listener"
    true_idle_respawned["engine"]["engine_owner_id"] = "respawned-engine-owner"
    true_idle_respawned["engine"]["native_worker_id"] = "respawned-native-worker"
    true_idle_process_observations = [
        {
            "phase": "true_idle_before",
            "observed_ns": 100,
            "snapshot": true_idle_before,
        },
        {
            "phase": "true_idle_after_wait",
            "observed_ns": 200,
            "snapshot": None,
        },
        {
            "phase": "true_idle_respawned",
            "observed_ns": 400,
            "snapshot": true_idle_respawned,
        },
    ]
    return true_idle_before, true_idle_respawned, true_idle_process_observations


def _true_idle_transitions(
    before: dict,
    respawned: dict,
) -> tuple[dict, str]:
    true_idle_before = before
    true_idle_respawned = respawned
    active_scheduler = {
        "query_capacity": 64,
        "query_depth": 1,
        "bulk_capacity": 64,
        "bulk_depth": 1,
        "active_request_count": 1,
        "lease_count": 1,
        "active_request_class": "query",
    }
    reclaimed_scheduler = {
        "query_capacity": 64,
        "query_depth": 0,
        "bulk_capacity": 64,
        "bulk_depth": 0,
        "active_request_count": 0,
        "lease_count": 0,
        "active_request_class": None,
    }
    materialized_sha256 = "c" * 64
    true_idle_transitions = {
        "anti_idle_work_observed": [{"values": active_scheduler}],
        "owner_preserved_across_idle_boundary": [
            {
                "values": {
                    "held_started_ns": 0,
                    "held_observed_ns": 60_000_000_000,
                    "contract_idle_timeout_ms": 60_000,
                    "server_instance_id": true_idle_before["process"][
                        "server_instance_id"
                    ],
                }
            }
        ],
        "anti_idle_work_reclaimed": [{"values": reclaimed_scheduler}],
        "true_idle_wait": [
            {
                "values": {
                    "server_idle_epoch_ns": 1,
                    "server_idle_elapsed_before_client_wait_ns": 59_000_000_000,
                    "client_wait_required_ns": 1_000_000_000,
                    "client_wait_elapsed_ns": 1_000_000_000,
                    "contract_idle_timeout_ms": 60_000,
                    "clock_boot_id": "boot-1",
                }
            }
        ],
        "idle_surfaces_exercised": [
            {
                "values": {
                    "diagnostic_count": 2,
                    "idle_connection_close_count": 2,
                    "last_diagnostic_client_elapsed_ns": 30_000_000_000,
                    "last_idle_connection_close_client_elapsed_ns": 30_000_000_000,
                }
            }
        ],
        "owner_absent_after_true_idle": [
            {
                "observed_ns": 225,
                "values": {
                    "old_server_instance_id": true_idle_before["process"][
                        "server_instance_id"
                    ]
                },
            }
        ],
        "server_respawned": [
            {
                "observed_ns": 450,
                "values": {
                    "new_server_instance_id": true_idle_respawned["process"][
                        "server_instance_id"
                    ],
                    "load_generation": 1,
                    "model_load_count": 1,
                    "materialized_model_sha256": materialized_sha256,
                    "materialized_reused": True,
                },
            }
        ],
    }
    return true_idle_transitions, materialized_sha256


def _verified_true_idle_fixture(
    first_snapshot: dict,
) -> TrueIdleFixture:
    before, respawned, true_idle_process_observations = _true_idle_snapshots(
        first_snapshot
    )
    true_idle_transitions, materialized_sha256 = _true_idle_transitions(
        before, respawned
    )
    # Mirrors the driver's post-#1420 invocation shape: one plain query causes
    # the respawn inside the proof window, and the resident-identity re-check
    # starts only after the replacement engine is witnessed (observed_ns 400),
    # so it must stay outside the consentless-respawn proof window.
    true_idle_invocations = [
        {
            "operation": "query",
            "started_ns": 250,
            "finished_ns": 350,
            "exit_code": 0,
            "termination": "exited",
        },
        {
            "operation": "query",
            "started_ns": 10,
            "finished_ns": 20,
            "exit_code": 0,
            "termination": "exited",
        },
        {
            "operation": "resident_identity",
            "started_ns": 420,
            "finished_ns": 440,
            "exit_code": 0,
            "termination": "exited",
        },
    ]
    true_idle_assertions = derive_scenario_assertions(
        "true_idle_respawn",
        observations_by_kind=true_idle_transitions,
        process_observations=true_idle_process_observations,
        invocations=true_idle_invocations,
        same_account={},
        materialization={
            "sha256": materialized_sha256,
            "reused_on_rejoin": False,
        },
    )
    require(
        all(true_idle_assertions.values()),
        "cold first-use state contaminated replacement materialization proof",
    )
    return TrueIdleFixture(
        transitions=true_idle_transitions,
        process_observations=true_idle_process_observations,
        invocations=true_idle_invocations,
        materialized_sha256=materialized_sha256,
    )


def _materialization_binding_hostiles(fixture: TrueIdleFixture) -> None:
    true_idle_transitions = fixture.transitions
    true_idle_process_observations = fixture.process_observations
    true_idle_invocations = fixture.invocations
    materialized_sha256 = fixture.materialized_sha256
    for field, hostile_value in (
        ("materialized_reused", False),
        ("materialized_model_sha256", "d" * 64),
    ):
        hostile_transitions = json.loads(json.dumps(true_idle_transitions))
        hostile_transitions["server_respawned"][0]["values"][field] = hostile_value
        try:
            derive_scenario_assertions(
                "true_idle_respawn",
                observations_by_kind=hostile_transitions,
                process_observations=true_idle_process_observations,
                invocations=true_idle_invocations,
                same_account={},
                materialization={
                    "sha256": materialized_sha256,
                    "reused_on_rejoin": False,
                },
            )
        except ProofFailure as error:
            require(
                str(error)
                == "qualification scenario true_idle_respawn raw evidence failed assertions: verified_materialization_reused",
                f"hostile true-idle {field} changed its exact failure",
            )
        else:
            raise ProofFailure(f"hostile true-idle {field} escaped replacement binding")


def _temporal_ordering_hostiles(fixture: TrueIdleFixture) -> None:
    true_idle_transitions = fixture.transitions
    true_idle_process_observations = fixture.process_observations
    true_idle_invocations = fixture.invocations
    materialized_sha256 = fixture.materialized_sha256
    historical_only_invocations = json.loads(json.dumps(true_idle_invocations[1:]))
    try:
        derive_scenario_assertions(
            "true_idle_respawn",
            observations_by_kind=true_idle_transitions,
            process_observations=true_idle_process_observations,
            invocations=historical_only_invocations,
            same_account={},
            materialization={
                "sha256": materialized_sha256,
                "reused_on_rejoin": False,
            },
        )
    except ProofFailure as error:
        require(
            str(error)
            == "qualification scenario true_idle_respawn raw evidence failed assertions: next_product_operation_respawns_without_consent",
            "historical true-idle query changed its exact temporal failure",
        )
    else:
        raise ProofFailure("historical query satisfied true-idle respawn proof")
    failed_then_successful_invocations = [
        {
            "operation": "query",
            "started_ns": 230,
            "finished_ns": 240,
            "exit_code": 1,
            "termination": "exited",
        },
        *true_idle_invocations,
    ]
    try:
        derive_scenario_assertions(
            "true_idle_respawn",
            observations_by_kind=true_idle_transitions,
            process_observations=true_idle_process_observations,
            invocations=failed_then_successful_invocations,
            same_account={},
            materialization={
                "sha256": materialized_sha256,
                "reused_on_rejoin": False,
            },
        )
    except ProofFailure as error:
        require(
            str(error)
            == "qualification scenario true_idle_respawn raw evidence failed assertions: next_product_operation_respawns_without_consent",
            "failed first true-idle query changed its exact failure",
        )
    else:
        raise ProofFailure("failed first query was hidden by a later respawn success")
    premature_identity_invocations = json.loads(json.dumps(true_idle_invocations))
    premature_identity_invocations[2]["started_ns"] = 300
    premature_identity_invocations[2]["finished_ns"] = 320
    try:
        derive_scenario_assertions(
            "true_idle_respawn",
            observations_by_kind=true_idle_transitions,
            process_observations=true_idle_process_observations,
            invocations=premature_identity_invocations,
            same_account={},
            materialization={
                "sha256": materialized_sha256,
                "reused_on_rejoin": False,
            },
        )
    except ProofFailure as error:
        require(
            str(error)
            == "qualification scenario true_idle_respawn raw evidence failed assertions: next_product_operation_respawns_without_consent",
            "premature identity probe changed its exact temporal failure",
        )
    else:
        raise ProofFailure(
            "identity probe inside the consentless-respawn window was accepted"
        )
    historical_respawn_transition = json.loads(json.dumps(true_idle_transitions))
    historical_respawn_transition["server_respawned"][0]["observed_ns"] = 150
    try:
        derive_scenario_assertions(
            "true_idle_respawn",
            observations_by_kind=historical_respawn_transition,
            process_observations=true_idle_process_observations,
            invocations=true_idle_invocations,
            same_account={},
            materialization={
                "sha256": materialized_sha256,
                "reused_on_rejoin": False,
            },
        )
    except ProofFailure as error:
        require(
            str(error)
            == "qualification scenario true_idle_respawn raw evidence failed assertions: next_product_operation_respawns_without_consent",
            "historical true-idle respawn transition changed its temporal failure",
        )
    else:
        raise ProofFailure("historical respawn transition was accepted")


def _absence_cardinality_hostile(fixture: TrueIdleFixture) -> None:
    true_idle_transitions = fixture.transitions
    true_idle_process_observations = fixture.process_observations
    true_idle_invocations = fixture.invocations
    materialized_sha256 = fixture.materialized_sha256
    duplicate_absence = json.loads(json.dumps(true_idle_process_observations))
    duplicate_absence.insert(
        -1,
        {
            "phase": "true_idle_after_wait",
            "observed_ns": 225,
            "snapshot": None,
        },
    )
    try:
        derive_scenario_assertions(
            "true_idle_respawn",
            observations_by_kind=true_idle_transitions,
            process_observations=duplicate_absence,
            invocations=true_idle_invocations,
            same_account={},
            materialization={
                "sha256": materialized_sha256,
                "reused_on_rejoin": False,
            },
        )
    except ProofFailure as error:
        require(
            str(error) == "true idle must retain exactly one absent-owner witness",
            "duplicate true-idle absence changed its cardinality failure",
        )
    else:
        raise ProofFailure("duplicate true-idle absence witness was accepted")


def run_true_idle_self_tests(server: ServerIdentityFixture) -> None:
    fixture = _verified_true_idle_fixture(server.first_snapshot)
    _materialization_binding_hostiles(fixture)
    _temporal_ordering_hostiles(fixture)
    _absence_cardinality_hostile(fixture)


# The idle boundary the harness has to respect, as opposed to the one the
# product measures. These budgets stand in for the real ones: the property being
# pinned is that a control fails fast and bounded, and reproducing that must not
# cost every `--self-test` run a real 90s wait.
_UNREACHED_CONTROL_TIMEOUT_SECS = 30
_ATTRIBUTION_BUDGET_SECS = 8
_SELF_TEST_NONCE = "self-test-nonce"
# Callers whose completion proves the product did admitted embedding work, which
# is the only thing that restarts the server's idle window. `establish` is the
# callable `send_control_to_resident_server` is handed for exactly that purpose.
_RESIDENCY_ESTABLISHING = {
    "ensure_resident_qualification_server",
    "run_embedding_qualification_query_worker",
    "run_publication_replacement_worker",
    "establish",
}
_CONTROL_SENDERS = {
    "send_server_qualification_control",
    "send_control_to_resident_server",
}
# Producers that hold a real handle on a real process, so their `exited()` can
# fail a wait. Everything else in `event_producer_liveness` answers "not exited"
# forever by design.
_PINNING_PRODUCERS = {
    "ChildProcessProducer",
    "NativeProcessProducer",
}


def _dead_producer(label: str) -> ChildProcessProducer:
    process = subprocess.Popen(
        [sys.executable, "-c", "raise SystemExit(9)"],
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
    )
    process.wait(timeout=60)
    return ChildProcessProducer(process, label, "answering a self-test control")


def _reaped_pid() -> int:
    """A pid that is provably not a live server, on every proof platform.

    ``NativeProcessProducer`` reports this pid as gone whichever way the host
    answers: the process is reaped, so Linux has no ``/proc`` entry, macOS
    ``proc_pidinfo`` and Windows ``OpenProcess`` both fail outright, and a
    recycled pid answers with a start identity that cannot match the pinned
    one. No branch of that probe can call it running.
    """
    process = subprocess.Popen([sys.executable, "-c", "raise SystemExit(0)"])
    process.wait(timeout=60)
    return process.pid


# A stand-in for `codestory-cli internal-embedding-qualification-worker`, with
# the one clause of the real worker that decides whether a tolerated respawn can
# run at all: it refuses to overwrite an existing output rather than reuse it
# (`embedding_qualification_output_exists`, worker.rs). A respawn test that
# stubs the establishing callable instead of the worker cannot see that clause,
# and would pass over a replacement that could never be established.
_WORKER_STUB = '''\
import json
import os
import sys
from pathlib import Path

argv = sys.argv[1:]
if not argv or argv[0] != "internal-embedding-qualification-worker":
    sys.stderr.write("unexpected subcommand: " + " ".join(argv) + "\\n")
    raise SystemExit(2)
request_path = Path(argv[argv.index("--request") + 1])
output_path = Path(argv[argv.index("--output") + 1])
if output_path.exists():
    sys.stderr.write("embedding_qualification_output_exists\\n")
    raise SystemExit(1)
request = json.loads(request_path.read_text(encoding="utf-8"))
counter = Path(os.environ["SELF_TEST_WORKER_INVOCATIONS"])
invocation = 1 + (
    json.loads(counter.read_text(encoding="utf-8")) if counter.exists() else 0
)
counter.write_text(json.dumps(invocation), encoding="utf-8")
if os.environ.get("SELF_TEST_FAIL_INVOCATION") == str(invocation):
    sys.stderr.write("self_test_worker_refused\\n")
    raise SystemExit(3)
if invocation == 1:
    # The server this query made resident then exits on true_idle before the
    # control reaches it, exactly as the frozen contract permits.
    snapshot = {
        "process": {
            "pid": int(os.environ["SELF_TEST_IDLED_OUT_PID"]),
            "process_start_id": "self-test-idled-out",
        }
    }
else:
    snapshot = {"process": None}
    with open(os.environ["SELF_TEST_EVENTS"], "a", encoding="utf-8") as handle:
        handle.write(
            json.dumps(
                {
                    "schema_version": 1,
                    "sequence": 1,
                    "action": "snapshot",
                    "status": "completed",
                    "snapshot": snapshot,
                },
                sort_keys=True,
            )
            + "\\n"
        )
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
                "final_snapshot": snapshot,
            },
        }
    ),
    encoding="utf-8",
)
'''


def _worker_stub_cli(directory: Path) -> Path:
    """The stub as something the harness can invoke exactly as it invokes a CLI."""
    script = directory / "worker-stub.py"
    script.write_text(_WORKER_STUB, encoding="utf-8")
    if os.name == "nt":
        cli = directory / "worker-stub.cmd"
        cli.write_text(
            f'@echo off\r\n"{sys.executable}" "{script}" %*\r\n',
            encoding="utf-8",
        )
        return cli
    cli = directory / "worker-stub"
    # `#!/bin/sh` stays inside every platform's shebang length limit; the
    # interpreter path, which does not, lives in the body instead.
    cli.write_text(
        f'#!/bin/sh\nexec "{sys.executable}" "{script}" "$@"\n',
        encoding="utf-8",
    )
    cli.chmod(0o700)
    return cli


def _answered(path: Path) -> None:
    path.write_text(
        json.dumps(
            {
                "schema_version": 1,
                "sequence": 1,
                "action": "snapshot",
                "status": "completed",
                "snapshot": {"process": {"pid": 4242, "process_start_id": "self-test"}},
            },
            sort_keys=True,
        )
        + "\n",
        encoding="utf-8",
    )


def _the_control_budget_is_the_servers_own_idle_budget() -> None:
    """A control may not outlive the server budget that decides its answer."""
    constants = json.loads(SERVER_CONSTANT_SET.read_text(encoding="utf-8"))
    frozen = constants.get("fixed_contract_values", {}).get("idle_timeout_ms")
    require(
        frozen == PER_USER_EMBEDDING_SERVER_IDLE_TIMEOUT_MS,
        "the harness idle timeout drifted from the frozen constant set"
        f" ({frozen!r} vs {PER_USER_EMBEDDING_SERVER_IDLE_TIMEOUT_MS!r}): the"
        " harness mirrors that constant and never chooses it",
    )
    require(
        SERVER_QUALIFICATION_CONTROL_TIMEOUT_SECS
        > PER_USER_EMBEDDING_SERVER_IDLE_TIMEOUT_MS // 1000,
        "the control budget must outlast the idle timeout it is bounding",
    )
    require(
        control_timeout_secs(1800) == SERVER_QUALIFICATION_CONTROL_TIMEOUT_SECS,
        "a qualification control was allowed to spend the whole proof budget,"
        " which turns a deterministic deadlock into an anonymous timeout",
    )
    require(
        control_timeout_secs(5) == 5 and control_timeout_secs(0) == 1,
        "the control budget stopped honouring a shorter proof budget",
    )


def _an_established_server_that_exits_is_replaced_exactly_once() -> None:
    """A legitimate idle exit costs one respawn, never the run and never a loop."""
    with tempfile.TemporaryDirectory(
        prefix="codestory-idle-boundary-self-test-"
    ) as raw:
        directory = Path(raw)
        nonce = "self-test-nonce"
        events = directory / f"{nonce}.events.jsonl"
        attempts: list[int] = []

        def establish(attempt: int):
            attempts.append(attempt)
            if attempt == 1:
                return _dead_producer("the self-test server that idled out")
            _answered(events)
            return UnobservedProducer(
                "the self-test replacement server",
                "answering the re-issued control",
                "is a self-test stand-in",
            )

        started = time.monotonic()
        event = send_control_to_resident_server(
            directory,
            nonce,
            sequence=1,
            action="snapshot",
            timeout=_UNREACHED_CONTROL_TIMEOUT_SECS,
            establish=establish,
        )
        elapsed = time.monotonic() - started
        require(
            event.get("sequence") == 1 and event.get("status") == "completed",
            "a tolerated respawn lost the control it re-issued",
        )
        require(
            attempts == [1, 2],
            f"a lost server was re-established as {attempts}, not exactly once"
            " under a distinguishable second attempt",
        )
        require(
            elapsed < _ATTRIBUTION_BUDGET_SECS,
            f"a lost server burned {elapsed:.1f}s before its replacement was"
            " established, instead of failing its wait fast",
        )
        require(
            not (directory / f"{nonce}.command.json").exists(),
            "a re-issued control left a command file behind",
        )


def _a_second_lost_server_fails_closed() -> None:
    """Tolerating a respawn must not become tolerating anything."""
    with tempfile.TemporaryDirectory(
        prefix="codestory-idle-boundary-self-test-"
    ) as raw:
        directory = Path(raw)
        nonce = "self-test-nonce"
        attempts: list[int] = []

        def establish(attempt: int):
            attempts.append(attempt)
            return _dead_producer(f"the self-test server {attempt}")

        started = time.monotonic()
        try:
            send_control_to_resident_server(
                directory,
                nonce,
                sequence=1,
                action="snapshot",
                timeout=_UNREACHED_CONTROL_TIMEOUT_SECS,
                establish=establish,
            )
        except ProofFailure as error:
            elapsed = time.monotonic() - started
            message = str(error)
            require(
                attempts == [1, 2],
                f"a doomed control was established as {attempts} instead of"
                " failing closed after one tolerated respawn",
            )
            require(
                elapsed < _ATTRIBUTION_BUDGET_SECS,
                f"a doomed control spent {elapsed:.1f}s before failing closed",
            )
            require(
                "lost its resident server twice" in message
                and "sequence=1 action=snapshot" in message
                and "the self-test server 1" in message
                and "exited with code 9" in message,
                f"a doubly lost control did not name both servers: {message}",
            )
        else:
            raise ProofFailure("a control with no server at all completed")


def _a_live_servers_refusal_is_never_retried_away() -> None:
    """Only a lost server earns a retry; a real answer is a real answer."""
    with tempfile.TemporaryDirectory(
        prefix="codestory-idle-boundary-self-test-"
    ) as raw:
        directory = Path(raw)
        nonce = "self-test-nonce"
        (directory / f"{nonce}.events.jsonl").write_text(
            json.dumps(
                {
                    "schema_version": 1,
                    "sequence": 1,
                    "action": "snapshot",
                    "status": "rejected",
                },
                sort_keys=True,
            )
            + "\n",
            encoding="utf-8",
        )
        attempts: list[int] = []

        def establish(attempt: int):
            attempts.append(attempt)
            return UnobservedProducer(
                "the self-test live server",
                "answering the control",
                "is a self-test stand-in that never exits",
            )

        try:
            send_control_to_resident_server(
                directory,
                nonce,
                sequence=1,
                action="snapshot",
                timeout=_UNREACHED_CONTROL_TIMEOUT_SECS,
                establish=establish,
            )
        except ProofFailure as error:
            require(
                attempts == [1],
                "a live server's refused control was retried, which would hide"
                " a real product failure behind a respawn",
            )
            require(
                "embedding qualification control snapshot failed" in str(error),
                f"a refused control lost its own failure: {error}",
            )
        else:
            raise ProofFailure("a refused qualification control was accepted")


def _stubbed_residency(root: Path, *, fail_invocation: int | None = None):
    """The real establishing path, wired to a worker stub instead of the CLI.

    Returns the qualification directory, the stub's invocation counter, the
    labels each attempt asked for, and the ``establish`` callable itself -- the
    real `ensure_resident_qualification_server`, not a stand-in for it.
    """
    private_root = root / "qualification-suite"
    private_root.mkdir(mode=0o700)
    project = root / "project"
    project.mkdir()
    cli = _worker_stub_cli(root)
    invocations = root / "worker-stub-invocations.json"
    env = {
        **os.environ,
        "SELF_TEST_WORKER_INVOCATIONS": str(invocations),
        "SELF_TEST_WORKER_SCHEMA_VERSION": str(
            EMBEDDING_QUALIFICATION_WORKER_SCHEMA_VERSION
        ),
        "SELF_TEST_IDLED_OUT_PID": str(_reaped_pid()),
        "SELF_TEST_EVENTS": str(private_root / f"{_SELF_TEST_NONCE}.events.jsonl"),
    }
    if fail_invocation is not None:
        env["SELF_TEST_FAIL_INVOCATION"] = str(fail_invocation)
    labels: list[str] = []

    def establish(attempt: int):
        labels.append(f"self-test-residency-{attempt}")
        return ensure_resident_qualification_server(
            cli,
            env,
            project,
            private_root,
            _SELF_TEST_NONCE,
            executable_sha256="a" * 64,
            timeout=_UNREACHED_CONTROL_TIMEOUT_SECS,
            activity="answering the self-test control",
            label=labels[-1],
        )

    return private_root, invocations, labels, establish


def _a_respawn_runs_through_the_real_worker_primitive() -> None:
    """The tolerated respawn has to run, not merely be written down.

    The other respawn tests hand `send_control_to_resident_server` a stand-in
    callable, which proves the control flow and nothing about the step that
    callable stands for. Residency is established by running the embedding
    qualification worker, and the worker refuses to overwrite an output file, so
    a second attempt that writes where the first one wrote cannot start -- the
    tolerance would be unreachable in production while every stubbed test still
    passed. This drives the real `ensure_resident_qualification_server` twice,
    through a worker that keeps that refusal, so the second attempt's evidence
    has to land somewhere of its own for the control to complete.
    """
    with tempfile.TemporaryDirectory(
        prefix="codestory-idle-boundary-self-test-"
    ) as raw:
        private_root, invocations, labels, establish = _stubbed_residency(Path(raw))
        started = time.monotonic()
        event = send_control_to_resident_server(
            private_root,
            _SELF_TEST_NONCE,
            sequence=1,
            action="snapshot",
            timeout=_UNREACHED_CONTROL_TIMEOUT_SECS,
            establish=establish,
        )
        elapsed = time.monotonic() - started
        require(
            event.get("sequence") == 1 and event.get("status") == "completed",
            "a respawn established through the real worker lost its control",
        )
        require(
            labels == ["self-test-residency-1", "self-test-residency-2"],
            f"the two residency attempts were not distinguishable: {labels}",
        )
        require(
            json.loads(invocations.read_text(encoding="utf-8")) == 2,
            "the replacement server was never actually established: the worker"
            " that establishes residency did not run a second time",
        )
        for label in labels:
            require(
                (private_root / f"publication-{label}-worker-output.json").is_file(),
                f"residency attempt {label} left no evidence of its own, so one"
                " attempt overwrote or inherited the other's",
            )
        require(
            elapsed < _ATTRIBUTION_BUDGET_SECS,
            f"a lost server burned {elapsed:.1f}s before its replacement ran",
        )


def _a_replacement_that_cannot_be_established_names_the_loss() -> None:
    """A replacement that never starts is still the lost server's failure.

    The re-establishment happens after the first control has already failed, so
    a failure there used to propagate as itself: an exit code from a worker,
    with no mention of the server whose loss asked for it. The report has to
    name what was being replaced, or it reads as an unrelated worker fault at a
    point in the run where nobody would look for a lost server.
    """
    with tempfile.TemporaryDirectory(
        prefix="codestory-idle-boundary-self-test-"
    ) as raw:
        private_root, _invocations, _labels, establish = _stubbed_residency(
            Path(raw),
            fail_invocation=2,
        )
        try:
            send_control_to_resident_server(
                private_root,
                _SELF_TEST_NONCE,
                sequence=1,
                action="snapshot",
                timeout=_UNREACHED_CONTROL_TIMEOUT_SECS,
                establish=establish,
            )
        except ProofFailure as error:
            message = str(error)
            require(
                "no replacement server could be established" in message
                and "sequence=1 action=snapshot" in message
                and SERVER_PRODUCER_LABEL in message,
                f"an unestablishable replacement lost its attribution: {message}",
            )
        else:
            raise ProofFailure(
                "a control whose replacement never started reported success"
            )


def _a_reused_worker_label_is_refused_before_the_worker_runs() -> None:
    """The collision that would silently disarm the respawn, named at its cause."""
    with tempfile.TemporaryDirectory(
        prefix="codestory-idle-boundary-self-test-"
    ) as raw:
        root = Path(raw)
        private_root = root / "qualification-suite"
        private_root.mkdir(mode=0o700)
        (private_root / "publication-taken-worker-output.json").write_text(
            "{}", encoding="utf-8"
        )
        try:
            ensure_resident_qualification_server(
                # Nothing here is runnable: the reuse has to be refused before
                # the worker is invoked, so reaching the worker is the failure.
                root / "never-invoked-cli",
                dict(os.environ),
                root,
                private_root,
                _SELF_TEST_NONCE,
                executable_sha256="a" * 64,
                timeout=_UNREACHED_CONTROL_TIMEOUT_SECS,
                activity="answering a self-test control",
                label="taken",
            )
        except ProofFailure as error:
            require(
                "was already used in this run" in str(error)
                and "publication-taken-worker-output.json" in str(error),
                f"a reused worker label lost its own attribution: {error}",
            )
        except OSError as error:
            raise ProofFailure(
                "a worker invocation that could never complete was allowed to"
                f" start: the harness reached the CLI itself ({error})"
            ) from error
        else:
            raise ProofFailure(
                "a worker invocation that could never complete was allowed to start"
            )


def _call_name(node: ast.Call) -> str | None:
    if isinstance(node.func, ast.Name):
        return node.func.id
    return getattr(node.func, "attr", None)


_NESTED_SCOPES = (ast.FunctionDef, ast.AsyncFunctionDef, ast.Lambda, ast.ClassDef)
# Fields whose contents run only on some paths through their parent. A residency
# call inside one cannot vouch for a control outside it: an `if` that was not
# taken, a handler that did not fire, and a loop that ran zero times all leave
# the server exactly as absent as no call at all.
_CONDITIONAL_FIELDS: dict[type, frozenset[str]] = {
    ast.If: frozenset({"body", "orelse"}),
    ast.IfExp: frozenset({"body", "orelse"}),
    ast.While: frozenset({"body", "orelse"}),
    ast.For: frozenset({"body", "orelse"}),
    ast.AsyncFor: frozenset({"body", "orelse"}),
    # `body` and `finalbody` are on every path that reaches what follows the
    # statement; `handlers` and `orelse` are not.
    ast.Try: frozenset({"handlers", "orelse"}),
    ast.ExceptHandler: frozenset({"body"}),
}
if hasattr(ast, "TryStar"):  # pragma: no cover - version dependent
    _CONDITIONAL_FIELDS[ast.TryStar] = frozenset({"handlers", "orelse"})


def _guarded_calls(function: ast.AST) -> list[tuple[int, str, ast.Call, bool]]:
    """Every residency and control call in one function body, in source order.

    Each mark carries whether it is reached only on some paths. Two kinds of
    call are excluded outright rather than marked: anything inside a nested
    function, lambda, or class, because that body runs when its own caller runs
    and not where it is written. The establishing callable a control is handed
    is written exactly there, and is audited as the control's own argument.
    """
    marks: list[tuple[int, int, str, ast.Call, bool]] = []

    def visit(node: ast.AST, *, conditional: bool) -> None:
        if node is not function and isinstance(node, _NESTED_SCOPES):
            return
        if isinstance(node, ast.Call):
            name = _call_name(node)
            if name in _RESIDENCY_ESTABLISHING:
                marks.append(
                    (node.lineno, node.col_offset, "residency", node, conditional)
                )
            elif name in _CONTROL_SENDERS:
                marks.append(
                    (node.lineno, node.col_offset, "control", node, conditional)
                )
        branching = _CONDITIONAL_FIELDS.get(type(node), frozenset())
        for field, value in ast.iter_fields(node):
            children = value if isinstance(value, list) else [value]
            for child in children:
                if isinstance(child, ast.AST):
                    visit(child, conditional=conditional or field in branching)

    visit(function, conditional=False)
    return [
        (lineno, kind, call, conditional)
        for lineno, _column, kind, call, conditional in sorted(
            marks, key=lambda mark: mark[:2]
        )
    ]


def _keyword(call: ast.Call, name: str) -> ast.expr | None:
    return next(
        (entry.value for entry in call.keywords if entry.arg == name),
        None,
    )


def _establishes_its_own_residency(call: ast.Call) -> bool:
    """A control that is handed the callable that makes a server resident.

    Positional order cannot decide this one: the establishing call is written
    inside the argument list, so it reads as "after" the control it precedes.
    The contract is the argument itself, and the body that consumes it is
    audited on its own terms.

    The argument has to name something that establishes residency. Merely being
    present and non-``None`` would let a callable that does no embedding work --
    ``lambda: None`` is the honest example -- satisfy the one guard standing
    between this harness and the deadlock it just removed.
    """
    establish = _keyword(call, "establish")
    if establish is None:
        return False
    return any(
        isinstance(node, ast.Name) and node.id in _RESIDENCY_ESTABLISHING
        for node in ast.walk(establish)
    )


def _pins_a_live_producer(call: ast.Call) -> bool:
    """A producer whose exit actually fails the wait it guards.

    An allowlist, not a denylist. ``UnobservedProducer`` says outright that it
    observes nothing, and ``ObservationalProducer`` deliberately reports another
    producer's state "without ever failing a wait on it"; both inherit
    ``exited() -> False``, so a control holding either is exactly as deadlocked
    as a control holding none. Naming the two producers that do pin a process
    keeps the next non-pinning producer out by default, instead of admitting it
    until someone remembers to exclude it here.
    """
    producer = _keyword(call, "producer")
    if producer is None:
        return False
    return any(
        isinstance(node, ast.Name) and node.id in _PINNING_PRODUCERS
        for node in ast.walk(producer)
    )


def _varies_with_its_attempt(call: ast.Call) -> bool:
    """Whether an inline establishing callable uses the attempt it is handed.

    Residency is established by running the embedding qualification worker, and
    the worker refuses to overwrite an output file. Two attempts that name their
    evidence identically therefore cannot both run: the replacement dies on its
    own output before it starts, and the run reports that instead of the server
    it lost. So the attempt number has to reach the evidence, not just the
    signature.

    Only an inline lambda is decided here, which is the shape the production
    call site uses; a named callable is a self-test stand-in that asserts the
    same property on its own results.
    """
    establish = _keyword(call, "establish")
    if not isinstance(establish, ast.Lambda):
        return True
    parameters = [argument.arg for argument in establish.args.args]
    if len(parameters) != 1:
        return False
    return any(
        isinstance(node, ast.Name) and node.id == parameters[0]
        for node in ast.walk(establish.body)
    )


def _every_control_is_issued_to_a_server_proven_resident() -> None:
    """Structural guard: the assumption this lane removed cannot come back.

    A control is answered only by a live server, and nothing in the control path
    starts one -- ``send_server_qualification_control`` writes a file and polls a
    log. So a control issued after an unbounded stretch of non-embedding work is
    a wait on a process that may already have exited for correct reasons, and
    the only fix available to the harness is to establish residency first.

    Every control call site must therefore either follow a residency-
    establishing call in its own function, or hold a pinned producer whose exit
    fails the wait immediately. A site with neither is the original defect.
    """
    package = REPOSITORY_ROOT / ".github" / "scripts" / "packaged_agent_proof"
    inspected = 0
    for source in sorted(package.glob("*.py")):
        tree = ast.parse(source.read_text(encoding="utf-8"), filename=str(source))
        for function in ast.walk(tree):
            if not isinstance(function, (ast.FunctionDef, ast.AsyncFunctionDef)):
                continue
            resident = False
            for lineno, kind, call, conditional in _guarded_calls(function):
                if kind == "residency":
                    # Only a call on every path to what follows it can vouch for
                    # a later control; a branch that may not run cannot.
                    resident = resident or not conditional
                    continue
                inspected += 1
                own = _establishes_its_own_residency(call)
                require(
                    resident or own or _pins_a_live_producer(call),
                    f"{source.name}:{lineno} in {function.name} issues an"
                    " embedding qualification control without first establishing"
                    " server residency and without pinning a live producer, so a"
                    " server that correctly exited on true_idle leaves the wait"
                    " deadlocked instead of attributed",
                )
                resident = resident or (own and not conditional)
    require(
        inspected >= 8,
        f"the qualification control audit inspected only {inspected} call sites",
    )


def _the_publication_fault_run_establishes_residency_before_its_first_control() -> None:
    """The exact site the Windows calibration cell deadlocked on."""
    source = (
        REPOSITORY_ROOT
        / ".github"
        / "scripts"
        / "packaged_agent_proof"
        / "publication_fault_run.py"
    )
    tree = ast.parse(source.read_text(encoding="utf-8"), filename=str(source))
    fault = next(
        (
            node
            for node in ast.walk(tree)
            if isinstance(node, ast.FunctionDef) and node.name == "_run_fault"
        ),
        None,
    )
    require(fault is not None, "publication_fault_run lost its _run_fault owner")
    marks = _guarded_calls(fault)
    require(len(marks) >= 3, "the publication fault run lost its three controls")
    first_lineno, first_kind, first_call, first_conditional = marks[0]
    require(
        not first_conditional
        and (first_kind == "residency" or _establishes_its_own_residency(first_call)),
        f"publication_fault_run.py:{first_lineno} issues this run's first"
        " embedding qualification control before establishing server residency:"
        " the baseline publication's only embedding work happens at its very"
        " start, so on a slow host that control is written long after the"
        " server's frozen idle budget has correctly expired, and it then waits"
        " on a process that already exited",
    )
    require(
        _varies_with_its_attempt(first_call),
        f"publication_fault_run.py:{first_lineno} establishes both residency"
        " attempts under one identity, so both would write the same worker"
        " request and output. The worker refuses to overwrite an output, so the"
        " replacement dies before it starts and the tolerated respawn -- a"
        " legitimate idle exit between the establishing query and the control --"
        " fails the run instead of surviving it",
    )
    for lineno, kind, call, _conditional in marks:
        if kind != "control":
            continue
        budget = _keyword(call, "timeout")
        require(
            isinstance(budget, ast.Name) and budget.id == "control_timeout",
            f"publication_fault_run.py:{lineno} lets one qualification control"
            " spend the whole proof budget instead of the budget that governs"
            " the server answering it",
        )


def run_idle_boundary_self_tests() -> None:
    _the_control_budget_is_the_servers_own_idle_budget()
    _an_established_server_that_exits_is_replaced_exactly_once()
    _a_second_lost_server_fails_closed()
    _a_live_servers_refusal_is_never_retried_away()
    _a_respawn_runs_through_the_real_worker_primitive()
    _a_replacement_that_cannot_be_established_names_the_loss()
    _a_reused_worker_label_is_refused_before_the_worker_runs()
    _every_control_is_issued_to_a_server_proven_resident()
    _the_publication_fault_run_establishes_residency_before_its_first_control()
