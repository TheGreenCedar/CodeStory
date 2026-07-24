"""Full-stack packaged proof self-test coordinator."""

from __future__ import annotations

import tempfile
from pathlib import Path

from .self_test_full_stack_calibration import run_calibration_self_tests
from .self_test_full_stack_external import run_external_evidence_self_tests
from .self_test_full_stack_fixture import build_full_stack_fixture
from .self_test_full_stack_manifest import run_hostile_manifest_self_tests
from .self_test_full_stack_qualification import (
    run_retained_qualification_self_tests,
)
from .self_test_server_identity import run_server_identity_self_tests
from .self_test_server_idle import run_true_idle_self_tests


def run_full_stack_self_tests() -> None:
    with tempfile.TemporaryDirectory() as raw:
        fixture = build_full_stack_fixture(Path(raw))
        server = run_server_identity_self_tests(fixture)
        run_true_idle_self_tests(server)
        measurement_contract = run_calibration_self_tests(fixture)
        external = run_external_evidence_self_tests(fixture)
        run_retained_qualification_self_tests(
            fixture,
            server,
            external,
            measurement_contract,
        )
        run_hostile_manifest_self_tests(fixture)
