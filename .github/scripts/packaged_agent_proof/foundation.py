#!/usr/bin/env python3
"""Shared proof contracts, paths, platform dependencies, and URI handling.

This module owns the immutable vocabulary used across the proof checker. Each
behavioral owner lives in its dedicated module.
"""

from __future__ import annotations

import argparse
from collections import Counter
import ctypes
import hashlib
import json
import math
import os
import queue
import re
import secrets
import shutil
import stat
import struct
import subprocess
import sys
import tarfile
import tempfile
import threading
import time
import zipfile
from pathlib import Path, PureWindowsPath
from urllib.parse import quote, unquote

from native_binary_contract import (
    NativeBinaryError,
    inspect_runtime_layout,
    runtime_artifact_role,
)

REPOSITORY_ROOT = Path(__file__).resolve().parents[3]


class ProofFailure(RuntimeError):
    pass


def require(condition: bool, message: str) -> None:
    if not condition:
        raise ProofFailure(message)


STATUS_URI = "codestory://status"
ENGINE_DIAGNOSTICS_URI = "codestory://diagnostics/retrieval-engine"
SERVER_PROOF_SCHEMA_VERSION = 1
QUALIFICATION_SCHEMA_VERSION = 1
NATIVE_SERVER_TEARDOWN_GRACE_MS = 60_000
PUBLICATION_FAULT_EVIDENCE_CONTRACT = "codestory-publication-lease-fault/v1"
FAULT_RECOVERY_CONSISTENCY_CONTRACT = "codestory-fault-recovery-search-consistency/v1"
RETRIEVAL_QUALITY_EVIDENCE_CONTRACT = "publishable-three-repeat-packet/v1"
MEMORY_EVIDENCE_CONTRACT = "codestory-five-process-memory/v1"
FAULT_RECOVERY_CONSISTENCY_CASES = 10
MIN_RETRIEVAL_QUALITY_REPEATS = 3
RELEASE_QUALITY_CORPUS_ID = "codestory-release-corpus-v1"
RELEASE_QUALITY_MODES = {"cold-cli": "cold_cli_packet"}
REQUIRED_HOLDOUT_TASK_FILES = {
    "axios-request-dispatch.task.json",
    "redis-server-event-loop.task.json",
    "ripgrep-search-pipeline.task.json",
}
CANDIDATE_PRODUCER_WORKFLOW_PATHS = {
    ".github/workflows/auto-release.yml",
    ".github/workflows/macos-metal-proof.yml",
    ".github/workflows/packaged-platform-pr.yml",
    ".github/workflows/packaged-platform-proof.yml",
    ".github/workflows/release.yml",
    ".github/workflows/windows-vulkan-proof.yml",
}


def project_resource_uri(base_uri: str, project: Path) -> str:
    value = str(project)
    if os.name == "nt":
        value = value.replace("\\", "/")
        if value.startswith("//?/UNC/"):
            value = f"//{value[len('//?/UNC/'):]}"
        elif value.startswith("//?/"):
            value = value[len("//?/"):]
    return f"{base_uri}?project={quote(value, safe='-._~')}"


def project_node_resource_uri(base_uri: str, node_id: str, project: Path) -> str:
    require(node_id != "", "project-bound node resource requires a node id")
    return project_resource_uri(
        f"{base_uri}/{quote(node_id, safe='-._~')}",
        project,
    )


def project_resource_uri_parts(uri: str) -> tuple[str, str] | None:
    base_uri, marker, encoded_project = uri.partition("?project=")
    if marker == "" or base_uri == "" or encoded_project == "":
        return None
    try:
        project = unquote(encoded_project, errors="strict")
    except UnicodeDecodeError:
        return None
    if quote(project, safe="-._~") != encoded_project:
        return None
    return base_uri, project


def resource_uri_matches(
    expected_uri: str,
    actual_uri: str,
    *,
    platform_name: str | None = None,
    samefile=None,
) -> bool:
    if actual_uri == expected_uri:
        return True
    if (os.name if platform_name is None else platform_name) != "nt":
        return False
    expected = project_resource_uri_parts(expected_uri)
    actual = project_resource_uri_parts(actual_uri)
    if expected is None or actual is None or expected[0] != actual[0]:
        return False
    if not (
        PureWindowsPath(expected[1]).is_absolute()
        and PureWindowsPath(actual[1]).is_absolute()
    ):
        return False
    identity_probe = os.path.samefile if samefile is None else samefile
    try:
        return identity_probe(Path(expected[1]), Path(actual[1]))
    except (OSError, ValueError):
        return False


EXTERNAL_QUALIFICATION_METRICS = {
    "retrieval_quality",
    "total_codestory_process_memory",
}
MEASUREMENT_PROTOCOL = (
    REPOSITORY_ROOT
    / "docs"
    / "testing"
    / "per-user-embedding-server-measurement-protocol.json"
)
CANDIDATE_QUALIFICATION_MATRIX_ALIASES = {
    "candidate_installed_windows_x64_cpu": {
        "source_cell_id": "installed_windows_x64_cpu",
        "source_host_class": "post_publish_windows_x64",
        "installation_source": "candidate",
        "cell": {
            "asset_target": "windows-x64",
            "proof_tier": "installed_runtime",
            "host_class": "premerge_candidate_windows_x64",
            "policy": "cpu_explicit",
            "backend": "cpu",
            "cache_state": "reused",
            "residency_state": "resident",
            "accelerator_claim": "none",
        },
    }
}
SERVER_PROTOCOL = MEASUREMENT_PROTOCOL.with_name("per-user-embedding-server-protocol.json")
SERVER_CONSTANT_SET = MEASUREMENT_PROTOCOL.with_name("per-user-embedding-server-constant-set.json")
HOLDOUT_TASK_ROOT = (
    REPOSITORY_ROOT / "benchmarks" / "tasks" / "holdout-retrieval"
)
DEFAULT_QUERY = "RuntimeContext"
DEFAULT_QUESTION = "Explain how CodeStory prepares retrieval."
SOFTWARE_ADAPTERS = ("llvmpipe", "lavapipe", "warp", "software rasterizer", "swiftshader")
LEGACY_TOKENS = (
    "llama-server",
    "repair-worker",
    "port-allocations",
    "native-embedding",
    "retrieval-sidecars",
    "sidecars",
    "owner.pid",
    "server.pid",
)
LEGACY_HELP_TOKENS = ("llama-server", "sidecar", "repair", "consent", "download")
NATIVE_MANIFEST_FILE = "codestory-native-manifest.json"
NATIVE_ENGINE_MARKER_PREFIX = "codestory-native-engine-v1|"
NATIVE_ENGINE_MARKER_SUFFIX = "|end"
SERVER_PROOF_MARKER_PREFIX = "codestory-embedding-server-proof-v1|"
SERVER_PROOF_MARKER_SUFFIX = "|end"
HEX_SHA256 = re.compile(r"^[0-9a-f]{64}$")
SERVER_LIFECYCLES = {
    "absent",
    "listening",
    "waking",
    "resident",
    "sleeping",
    "draining",
    "unreachable",
    "exited",
}
RETRY_CLASSES = {
    "none",
    "after_delay",
    "after_capacity_change",
    "after_server_change",
    "after_owner_idle",
    "same_rpc_once",
    "terminal",
}
REQUIRED_SERVER_SCENARIOS = {
    "cold_race",
    "mixed_queue",
    "client_death",
    "server_crash",
    "worker_stall",
    "true_idle_respawn",
    "incompatible_owner",
    "frozen_owner",
}
LOWER_TIER_NONCLAIMS = {
    "answer_quality",
    "release_readiness",
    "cross_user_sharing",
    "cross_session_sharing",
    "bounded_bulk_starvation",
    "whole_server_takeover",
    "linux_gpu_execution",
}
TARGET_CONTRACTS = {
    "linux-x64": {
        "binary_name": "codestory-cli",
        "binary_format": "elf",
        "target_triple": "x86_64-unknown-linux-gnu",
        "target_os": "linux",
        "target_arch": "x86_64",
        "compiled_backends": ["cpu", "vulkan"],
        "linkage": "dynamic",
        "backend_loading": "runtime-modules",
        "expected_protected_backend": None,
        "non_claim_reason": "linux_gpu_execution_is_not_a_release_claim",
    },
    "linux-arm64": {
        "binary_name": "codestory-cli",
        "binary_format": "elf",
        "target_triple": "aarch64-unknown-linux-gnu",
        "target_os": "linux",
        "target_arch": "aarch64",
        "compiled_backends": ["cpu", "vulkan"],
        "linkage": "dynamic",
        "backend_loading": "runtime-modules",
        "expected_protected_backend": None,
        "non_claim_reason": "linux_gpu_execution_is_not_a_release_claim",
    },
    "windows-x64": {
        "binary_name": "codestory-cli.exe",
        "binary_format": "pe",
        "target_triple": "x86_64-pc-windows-msvc",
        "target_os": "windows",
        "target_arch": "x86_64",
        "compiled_backends": ["cpu", "vulkan"],
        "linkage": "dynamic",
        "backend_loading": "runtime-modules",
        "expected_protected_backend": "vulkan",
        "non_claim_reason": None,
    },
    "windows-arm64": {
        "binary_name": "codestory-cli.exe",
        "binary_format": "pe",
        "target_triple": "aarch64-pc-windows-msvc",
        "target_os": "windows",
        "target_arch": "aarch64",
        "compiled_backends": ["cpu", "vulkan"],
        "linkage": "dynamic",
        "backend_loading": "runtime-modules",
        "expected_protected_backend": None,
        "non_claim_reason": "windows_arm64_accelerator_execution_is_not_protected",
    },
    "macos-x64": {
        "binary_name": "codestory-cli",
        "binary_format": "mach-o",
        "target_triple": "x86_64-apple-darwin",
        "target_os": "macos",
        "target_arch": "x86_64",
        "compiled_backends": ["cpu", "metal"],
        "linkage": "static",
        "backend_loading": "builtin",
        "expected_protected_backend": None,
        "non_claim_reason": "macos_x64_accelerator_execution_is_not_protected",
    },
    "macos-arm64": {
        "binary_name": "codestory-cli",
        "binary_format": "mach-o",
        "target_triple": "aarch64-apple-darwin",
        "target_os": "macos",
        "target_arch": "aarch64",
        "compiled_backends": ["cpu", "metal"],
        "linkage": "static",
        "backend_loading": "builtin",
        "expected_protected_backend": "metal",
        "non_claim_reason": None,
    },
}
