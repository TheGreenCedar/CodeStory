"""Contract-level packaged proof self-test coordinator."""

from .self_test_contract_scope import run_contract_scope_self_tests
from .self_test_resource_identity import run_resource_identity_self_tests


def run_contract_self_tests() -> None:
    run_contract_scope_self_tests()
    run_resource_identity_self_tests()
