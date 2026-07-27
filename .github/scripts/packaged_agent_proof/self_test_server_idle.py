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
"""

from __future__ import annotations

import ast
import json
import subprocess
import sys
import tempfile
import time
from pathlib import Path

from .event_producer_liveness import ChildProcessProducer, UnobservedProducer
from .foundation import (
    PER_USER_EMBEDDING_SERVER_IDLE_TIMEOUT_MS,
    REPOSITORY_ROOT,
    SERVER_CONSTANT_SET,
    SERVER_QUALIFICATION_CONTROL_TIMEOUT_SECS,
    ProofFailure,
    require,
)
from .publication_protocol import (
    control_timeout_secs,
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


def _dead_producer(label: str) -> ChildProcessProducer:
    process = subprocess.Popen(
        [sys.executable, "-c", "raise SystemExit(9)"],
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
    )
    process.wait(timeout=60)
    return ChildProcessProducer(process, label, "answering a self-test control")


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
        attempts: list[str] = []

        def establish():
            attempts.append("established")
            if len(attempts) == 1:
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
            attempts == ["established", "established"],
            f"a lost server was re-established {len(attempts)} time(s), not once",
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
        attempts: list[str] = []

        def establish():
            attempts.append("established")
            return _dead_producer(f"the self-test server {len(attempts)}")

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
                attempts == ["established", "established"],
                f"a doomed control retried {len(attempts)} time(s) instead of"
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
        attempts: list[str] = []

        def establish():
            attempts.append("established")
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
                attempts == ["established"],
                "a live server's refused control was retried, which would hide"
                " a real product failure behind a respawn",
            )
            require(
                "embedding qualification control snapshot failed" in str(error),
                f"a refused control lost its own failure: {error}",
            )
        else:
            raise ProofFailure("a refused qualification control was accepted")


def _call_name(node: ast.Call) -> str | None:
    if isinstance(node.func, ast.Name):
        return node.func.id
    return getattr(node.func, "attr", None)


def _guarded_calls(function: ast.AST) -> list[tuple[int, str, ast.Call]]:
    """Every residency and control call in one function, in source order."""
    marks: list[tuple[int, int, str, ast.Call]] = []
    for node in ast.walk(function):
        if not isinstance(node, ast.Call):
            continue
        name = _call_name(node)
        if name in _RESIDENCY_ESTABLISHING:
            marks.append((node.lineno, node.col_offset, "residency", node))
        elif name in _CONTROL_SENDERS:
            marks.append((node.lineno, node.col_offset, "control", node))
    return [
        (lineno, kind, call)
        for lineno, _column, kind, call in sorted(marks, key=lambda mark: mark[:2])
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
    """
    establish = _keyword(call, "establish")
    return establish is not None and not (
        isinstance(establish, ast.Constant) and establish.value is None
    )


def _pins_a_live_producer(call: ast.Call) -> bool:
    producer = _keyword(call, "producer")
    if producer is None:
        return False
    return not any(
        isinstance(node, ast.Name) and node.id == "UnobservedProducer"
        for node in ast.walk(producer)
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
            for lineno, kind, call in _guarded_calls(function):
                if kind == "residency":
                    resident = True
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
                resident = resident or own
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
    first_lineno, first_kind, first_call = marks[0]
    require(
        first_kind == "residency" or _establishes_its_own_residency(first_call),
        f"publication_fault_run.py:{first_lineno} issues this run's first"
        " embedding qualification control before establishing server residency:"
        " the baseline publication's only embedding work happens at its very"
        " start, so on a slow host that control is written long after the"
        " server's frozen idle budget has correctly expired, and it then waits"
        " on a process that already exited",
    )
    for lineno, kind, call in marks:
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
    _every_control_is_issued_to_a_server_proven_resident()
    _the_publication_fault_run_establishes_residency_before_its_first_control()
