"""Process-level packaged proof self-test coordinator."""

from .self_test_process_cleanup import run_process_cleanup_self_tests
from .self_test_process_capture import run_process_capture_self_tests
from .self_test_process_clock import run_process_clock_self_tests
from .self_test_process_deadline import run_process_deadline_self_tests
from .self_test_process_exit import run_process_exit_self_tests
from .self_test_process_identity import run_process_identity_self_tests


def run_process_self_tests() -> None:
    run_process_identity_self_tests()
    run_process_clock_self_tests()
    run_process_deadline_self_tests()
    run_process_exit_self_tests()
    run_process_cleanup_self_tests()
    run_process_capture_self_tests()
