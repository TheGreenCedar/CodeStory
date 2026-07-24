"""Public packaged-proof archive surface."""

from .archive_io import expected_archive_digest, find_cli, safe_target, unpack_archive
from .contracts import normalized_backend
from .native_contract_identity import (
    binary_markers,
    embedding_contract_digest,
    native_engine_markers,
    ordered_contract_digest,
    parse_native_build_identity,
    parse_server_proof_identity,
    server_proof_markers,
    verify_runtime_against_manifest,
)
from .native_manifest import load_native_manifest

__all__ = [
    "binary_markers",
    "embedding_contract_digest",
    "expected_archive_digest",
    "find_cli",
    "load_native_manifest",
    "native_engine_markers",
    "normalized_backend",
    "ordered_contract_digest",
    "parse_native_build_identity",
    "parse_server_proof_identity",
    "safe_target",
    "server_proof_markers",
    "unpack_archive",
    "verify_runtime_against_manifest",
]
