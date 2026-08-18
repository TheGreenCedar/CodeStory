"""Worker wire-contract lineage self-tests.

The publication replacement worker step hand-writes the packaged CLI worker's
wire request instead of compiling against the Rust contract, so a schema bump
in `codestory-retrieval` cannot break it at build time. These self-tests pin
the harness declaration to the Rust source of truth in this tree and drive the
replacement worker step against both wire versions, so the next contract bump
fails this PR-time lane instead of a calibration cell. The harness must keep
asserting the version its own tree compiled — never a version echoed by the
package or binary under test — so a stale runtime can never look
self-consistent.
"""

from __future__ import annotations

import json
import re
import tempfile
from pathlib import Path
from unittest.mock import patch

from . import publication_protocol
from .event_producer_liveness import EventProducer, NativeProcessProducer
from .foundation import (
    EMBEDDING_QUALIFICATION_WORKER_CONTRACT_SOURCE,
    EMBEDDING_QUALIFICATION_WORKER_SCHEMA_VERSION,
    ProofFailure,
    require,
)

_RUST_WORKER_SCHEMA_DECLARATION = re.compile(
    r"^pub const EMBEDDING_QUALIFICATION_WORKER_SCHEMA_VERSION: u32 = (\d+);$",
    re.MULTILINE,
)


def compiled_worker_schema_version() -> int:
    source = EMBEDDING_QUALIFICATION_WORKER_CONTRACT_SOURCE.read_text(
        encoding="utf-8"
    )
    declarations = _RUST_WORKER_SCHEMA_DECLARATION.findall(source)
    require(
        len(declarations) == 1,
        "the worker wire contract source must declare exactly one schema version",
    )
    return int(declarations[0])


def _source_lineage_tests() -> None:
    require(
        compiled_worker_schema_version()
        == EMBEDDING_QUALIFICATION_WORKER_SCHEMA_VERSION,
        "packaged-proof worker schema version disagrees with "
        "crates/codestory-retrieval/src/per_user_embedding/"
        "qualification_worker.rs; bump "
        "EMBEDDING_QUALIFICATION_WORKER_SCHEMA_VERSION in "
        "packaged_agent_proof/foundation.py in the same change as the Rust "
        "contract",
    )


def _worker_output(
    executable_sha256: str,
    schema_version: int,
    *,
    result_schema_version: int = 1,
) -> dict:
    return {
        "schema_version": schema_version,
        "executable_sha256": executable_sha256,
        "error": None,
        "result": {
            "schema_version": result_schema_version,
            "scenario": "query",
            "operations": [{"status": "ok", "error_code": None}],
        },
    }


def _drive_replacement_worker(output_document) -> dict:
    executable_sha256 = "a" * 64
    with tempfile.TemporaryDirectory() as root:
        private_root = Path(root) / "private"
        project = Path(root) / "project"
        project.mkdir()
        observed: dict = {}

        def fake_run(command, *, env, cwd, timeout):
            request_path = Path(command[command.index("--request") + 1])
            output_path = Path(command[command.index("--output") + 1])
            observed["request"] = json.loads(
                request_path.read_text(encoding="utf-8")
            )
            output_path.write_text(
                json.dumps(output_document(executable_sha256)) + "\n",
                encoding="utf-8",
            )
            return {"exit_code": 0, "stdout": "", "stderr": ""}

        predecessor = NativeProcessProducer(
            4242,
            (
                "windows:1"
                if publication_protocol.os.name == "nt"
                else (
                    "macos-proc:1:1"
                    if publication_protocol.sys.platform == "darwin"
                    else "linux:1"
                )
            ),
            "the self-test predecessor",
            "exiting after a self-test crash",
        )
        candidate = EventProducer(
            "the self-test candidate",
            "remaining paused during replacement",
        )
        class ExitedWaiter:
            def exited(self):
                return True

            def close(self):
                return None

        with (
            patch.object(publication_protocol, "run", fake_run),
            patch.object(
                publication_protocol,
                "ExactProcessExitWaiter",
                return_value=ExitedWaiter(),
            ),
        ):
            publication_protocol.run_publication_replacement_worker(
                Path(root) / "codestory-cli",
                {},
                project,
                private_root,
                "self-test-nonce",
                crash_event={
                    "action": "crash_server",
                    "status": "accepted",
                    "snapshot": {
                        "process": {
                            "pid": predecessor.pid,
                            "process_start_id": predecessor.process_start_id,
                        }
                    },
                },
                candidate_producer=candidate,
                executable_sha256=executable_sha256,
                timeout=5,
            )
        return observed["request"]


def _replacement_worker_wire_tests() -> None:
    compiled = compiled_worker_schema_version()
    request = _drive_replacement_worker(
        lambda executable_sha256: _worker_output(executable_sha256, compiled)
    )
    require(
        request.get("schema_version") == compiled,
        "publication replacement worker request drifted from the compiled "
        "worker wire contract",
    )
    for stale in (compiled - 1, compiled + 1):
        try:
            _drive_replacement_worker(
                lambda executable_sha256, stale=stale: _worker_output(
                    executable_sha256, stale
                )
            )
        except ProofFailure:
            pass
        else:
            raise ProofFailure(
                f"a schema_version={stale} replacement worker output was accepted"
            )
    wrong_inner = compiled if compiled != 1 else 2
    try:
        _drive_replacement_worker(
            lambda executable_sha256: _worker_output(
                executable_sha256, compiled, result_schema_version=wrong_inner
            )
        )
    except ProofFailure:
        pass
    else:
        raise ProofFailure(
            "the inner qualification result contract followed the outer "
            "worker wire version"
        )


def run_worker_schema_self_tests() -> None:
    _source_lineage_tests()
    _replacement_worker_wire_tests()
