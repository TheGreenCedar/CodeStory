"""Owner-oriented packaged-proof self-test aggregation."""

from .self_test_cli import run_cli_self_tests
from .self_test_contracts import run_contract_self_tests
from .self_test_full_stack import run_full_stack_self_tests
from .self_test_installation import run_installation_self_tests
from .self_test_process import run_process_self_tests
from .self_test_qualification import run_qualification_self_tests


def self_test() -> None:
    run_cli_self_tests()
    run_contract_self_tests()
    run_process_self_tests()
    run_qualification_self_tests()
    run_installation_self_tests()
    run_full_stack_self_tests()
    print("packaged per-user embedding server proof self-test passed")
