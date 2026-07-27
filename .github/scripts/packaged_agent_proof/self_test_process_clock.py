"""Suspend-inclusive clock sampling proof self-tests."""

from __future__ import annotations

import os
import sys

from .foundation import ProofFailure, require
from .process_memory_sampling import (
    WINDOWS_INTERRUPT_CLOCK_MODULES,
    resolve_windows_interrupt_clocks,
    suspend_clock_pair,
)

_CLOCK_EXPORTS = ("QueryUnbiasedInterruptTimePrecise", "QueryInterruptTimePrecise")
_EXPECTED_CLOCK_APIS = {
    "linux": ("CLOCK_MONOTONIC", "CLOCK_BOOTTIME"),
    "macos": ("mach_absolute_time", "mach_continuous_time"),
    "windows": _CLOCK_EXPORTS,
}


def _target_os() -> str:
    if os.name == "nt":
        return "windows"
    return "macos" if sys.platform == "darwin" else "linux"


class _FakeClockFunction:
    def __init__(self, module_name: str, export: str) -> None:
        self.module_name = module_name
        self.export = export


class _FakeRealtimeModule:
    def __init__(self, name: str, exports: tuple[str, ...]) -> None:
        for export in exports:
            setattr(self, export, _FakeClockFunction(name, export))


def _require_resolved_pair(pair, module_name: str, label: str) -> None:
    unbiased, inclusive = pair
    require(
        (unbiased.module_name, unbiased.export)
        == (module_name, "QueryUnbiasedInterruptTimePrecise")
        and (inclusive.module_name, inclusive.export)
        == (module_name, "QueryInterruptTimePrecise"),
        f"{label} resolved the clock pair from the wrong module",
    )
    for clock in (unbiased, inclusive):
        require(
            getattr(clock, "restype", 0) is None
            and len(getattr(clock, "argtypes", ())) == 1,
            f"{label} left the clock pair without void/out-parameter prototypes",
        )


def _run_host_clock_gate_leg() -> None:
    target_os = _target_os()
    first = suspend_clock_pair(target_os)
    second = suspend_clock_pair(target_os)
    require(
        first[2:] == _EXPECTED_CLOCK_APIS[target_os],
        f"{target_os} suspend clock pair reported unexpected APIs {first[2:]}",
    )
    require(
        second[2:] == first[2:],
        "suspend clock APIs changed between consecutive samples",
    )
    require(
        second[0] >= first[0] > 0 and second[1] >= first[1] > 0,
        "suspend clock pair moved backwards between consecutive samples",
    )
    try:
        suspend_clock_pair("freebsd")
    except ProofFailure as failure:
        require(
            "unsupported qualification clock target" in str(failure),
            "unsupported clock target failed with an unexpected error",
        )
    else:
        raise ProofFailure("unsupported clock target was not rejected")


def _run_resolver_preference_leg() -> None:
    requested: list[str] = []

    def load_module(name: str) -> _FakeRealtimeModule:
        requested.append(name)
        return _FakeRealtimeModule(name, _CLOCK_EXPORTS)

    pair = resolve_windows_interrupt_clocks(load_module)
    require(
        requested == [WINDOWS_INTERRUPT_CLOCK_MODULES[0]],
        "clock resolver did not prefer the realtime API set module",
    )
    _require_resolved_pair(pair, WINDOWS_INTERRUPT_CLOCK_MODULES[0], "clock resolver")


def _run_resolver_fallback_leg() -> None:
    requested: list[str] = []

    def load_module(name: str) -> _FakeRealtimeModule:
        requested.append(name)
        if name == WINDOWS_INTERRUPT_CLOCK_MODULES[0]:
            raise OSError(f"cannot load {name}")
        return _FakeRealtimeModule(name, _CLOCK_EXPORTS)

    pair = resolve_windows_interrupt_clocks(load_module)
    require(
        requested == list(WINDOWS_INTERRUPT_CLOCK_MODULES),
        "clock resolver skipped the API set before falling back",
    )
    _require_resolved_pair(
        pair, WINDOWS_INTERRUPT_CLOCK_MODULES[1], "clock resolver fallback"
    )

    def load_partial(name: str) -> _FakeRealtimeModule:
        if name == WINDOWS_INTERRUPT_CLOCK_MODULES[0]:
            return _FakeRealtimeModule(name, _CLOCK_EXPORTS[:1])
        return _FakeRealtimeModule(name, _CLOCK_EXPORTS)

    _require_resolved_pair(
        resolve_windows_interrupt_clocks(load_partial),
        WINDOWS_INTERRUPT_CLOCK_MODULES[1],
        "clock resolver with a partial API set export",
    )


def _run_resolver_fail_closed_leg() -> None:
    def load_nothing(name: str):
        raise OSError(f"cannot load {name}")

    def load_empty(name: str) -> _FakeRealtimeModule:
        return _FakeRealtimeModule(name, ())

    for label, load_module in (
        ("unloadable modules", load_nothing),
        ("modules without the exports", load_empty),
    ):
        try:
            resolve_windows_interrupt_clocks(load_module)
        except ProofFailure as failure:
            require(
                str(failure)
                == "Windows qualification host could not read unbiased interrupt time",
                f"clock resolver changed its fail-closed error for {label}",
            )
        else:
            raise ProofFailure(f"clock resolver did not fail closed for {label}")


def run_process_clock_self_tests() -> None:
    _run_host_clock_gate_leg()
    _run_resolver_preference_leg()
    _run_resolver_fallback_leg()
    _run_resolver_fail_closed_leg()
