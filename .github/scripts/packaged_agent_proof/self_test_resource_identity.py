"""Resource and native process identity contract self-tests."""

from __future__ import annotations

import json
from pathlib import Path

from .foundation import (
    ProofFailure,
    project_node_resource_uri,
    require,
    resource_uri_matches,
)
from .installation import run_parallel
from .process import extract_resource, require_native_process_start_identity

SHORT_WINDOWS_RESOURCE = (
    "codestory://diagnostics/retrieval-engine"
    "?project=C%3A%2FUsers%2FRUNNER~1%2FAppData%2FLocal%2FTemp%2Fproof"
)
LONG_WINDOWS_RESOURCE = (
    "codestory://diagnostics/retrieval-engine"
    "?project=C%3A%2FUsers%2Frunneradmin%2FAppData%2FLocal%2FTemp%2Fproof"
)
EXPECTED_WINDOWS_IDENTITY = (
    Path("C:/Users/RUNNER~1/AppData/Local/Temp/proof"),
    Path("C:/Users/runneradmin/AppData/Local/Temp/proof"),
)


def _resource_uri_tests() -> None:
    uri_project = Path("proof root")
    node_uri = project_node_resource_uri(
        "codestory://snippet",
        "node/id %1",
        uri_project,
    )
    require(
        node_uri == "codestory://snippet/node%2Fid%20%251?project=proof%20root",
        f"project-bound node resource URI encoding drifted: {node_uri}",
    )
    require(
        extract_resource(
            {
                "result": {
                    "contents": [
                        {
                            "uri": node_uri,
                            "text": json.dumps({"node": {"id": "node/id %1"}}),
                        }
                    ]
                }
            },
            node_uri,
        )
        == {"node": {"id": "node/id %1"}},
        "project-bound named resource extraction failed",
    )


def _windows_alias_acceptance_tests() -> None:
    identity_probes: list[tuple[Path, Path]] = []

    def same_windows_resource(left: Path, right: Path) -> bool:
        identity_probes.append((left, right))
        return True

    require(
        extract_resource(
            {
                "result": {
                    "contents": [
                        {
                            "uri": LONG_WINDOWS_RESOURCE,
                            "text": json.dumps({"native_alias": True}),
                        }
                    ]
                }
            },
            SHORT_WINDOWS_RESOURCE,
            platform_name="nt",
            samefile=same_windows_resource,
        )
        == {"native_alias": True}
        and identity_probes == [EXPECTED_WINDOWS_IDENTITY]
        and EXPECTED_WINDOWS_IDENTITY[0] != EXPECTED_WINDOWS_IDENTITY[1],
        "native-identical Windows project resource URI was rejected",
    )
    short_snippet = SHORT_WINDOWS_RESOURCE.replace(
        "codestory://diagnostics/retrieval-engine",
        "codestory://snippet/node%2Fid",
    )
    long_snippet = LONG_WINDOWS_RESOURCE.replace(
        "codestory://diagnostics/retrieval-engine",
        "codestory://snippet/node%2Fid",
    )
    snippet_identity_probes: list[tuple[Path, Path]] = []

    def same_windows_snippet(left: Path, right: Path) -> bool:
        snippet_identity_probes.append((left, right))
        return True

    require(
        resource_uri_matches(
            short_snippet,
            long_snippet,
            platform_name="nt",
            samefile=same_windows_snippet,
        )
        and snippet_identity_probes == [EXPECTED_WINDOWS_IDENTITY],
        "native-identical Windows snippet link URI was rejected",
    )


def _windows_alias_rejection_tests() -> None:
    identity_probes: list[tuple[Path, Path]] = []

    def same_windows_resource(left: Path, right: Path) -> bool:
        identity_probes.append((left, right))
        return True

    require(
        not resource_uri_matches(
            SHORT_WINDOWS_RESOURCE,
            LONG_WINDOWS_RESOURCE,
            platform_name="posix",
            samefile=same_windows_resource,
        )
        and not identity_probes,
        "Unix project resource matching accepted a different path spelling",
    )
    for hostile_resource, message in (
        (
            LONG_WINDOWS_RESOURCE.replace(
                "codestory://diagnostics/retrieval-engine",
                "codestory://status",
            ),
            "Windows project resource matching accepted a different resource base",
        ),
        (
            LONG_WINDOWS_RESOURCE.replace("%3A", "%3a"),
            "Windows project resource matching accepted noncanonical URI encoding",
        ),
        (
            LONG_WINDOWS_RESOURCE.replace("C%3A%2F", "relative%2F"),
            "Windows project resource matching accepted a relative project selector",
        ),
    ):
        require(
            not resource_uri_matches(
                SHORT_WINDOWS_RESOURCE,
                hostile_resource,
                platform_name="nt",
                samefile=same_windows_resource,
            )
            and not identity_probes,
            message,
        )
    require(
        not resource_uri_matches(
            SHORT_WINDOWS_RESOURCE,
            LONG_WINDOWS_RESOURCE,
            platform_name="nt",
            samefile=lambda _left, _right: False,
        ),
        "Windows project resource matching accepted a different native identity",
    )


def _resource_error_tests() -> None:
    def missing_windows_resource(_left: Path, _right: Path) -> bool:
        raise FileNotFoundError("missing project resource")

    require(
        not resource_uri_matches(
            SHORT_WINDOWS_RESOURCE,
            LONG_WINDOWS_RESOURCE,
            platform_name="nt",
            samefile=missing_windows_resource,
        ),
        "Windows project resource matching ignored an identity probe failure",
    )

    def fail_parallel(message: str) -> None:
        raise ProofFailure(message)

    try:
        run_parallel(
            {
                "z-task": lambda: fail_parallel("z failed"),
                "a-task": lambda: fail_parallel("a failed"),
            }
        )
    except ProofFailure as error:
        require(
            str(error)
            == "parallel qualification tasks failed: a-task: a failed; z-task: z failed",
            "parallel qualification failure aggregation is unstable",
        )
    else:
        raise ProofFailure("parallel qualification failures were ignored")


def _native_identity_format_tests() -> None:
    require(
        require_native_process_start_identity(
            "linux:1234", "linux", "Linux self-test identity"
        )
        == "linux:1234"
        and require_native_process_start_identity(
            "macos-proc:1234:5678", "macos", "macOS self-test identity"
        )
        == "macos-proc:1234:5678"
        and require_native_process_start_identity(
            "windows:504911232000000010",
            "windows",
            "Windows self-test identity",
        )
        == "windows:504911232000000010",
        "canonical process identity format self-test failed",
    )
    for target_os, hostile_identity in (
        ("linux", "boot-id:1234"),
        ("macos", "Thu Jul 17 12:00:00 2026"),
        ("windows", "2026-07-17T12:00:00Z"),
    ):
        try:
            require_native_process_start_identity(
                hostile_identity,
                target_os,
                f"hostile {target_os} identity",
            )
        except ProofFailure:
            pass
        else:
            raise ProofFailure(
                f"noncanonical {target_os} process identity format was accepted"
            )


def run_resource_identity_self_tests() -> None:
    _resource_uri_tests()
    _windows_alias_acceptance_tests()
    _windows_alias_rejection_tests()
    _resource_error_tests()
    _native_identity_format_tests()
