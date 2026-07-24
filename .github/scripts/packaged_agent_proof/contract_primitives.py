"""Primitive validation, digest, privacy, and JSON contracts."""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import re
import secrets
from pathlib import Path

from .foundation import HEX_SHA256, ProofFailure, require


def normalized_backend(value: object) -> str:
    backend = str(value or "").strip().lower()
    if backend == "mtl":
        return "metal"
    if backend.startswith("vulkan"):
        return "vulkan"
    return backend


def write_json(path: Path, value: object) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(
        json.dumps(value, indent=2, sort_keys=True) + "\n", encoding="utf-8"
    )


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def canonical_sha256(value: object) -> str:
    payload = json.dumps(
        value, sort_keys=True, separators=(",", ":"), ensure_ascii=False
    )
    return hashlib.sha256(payload.encode("utf-8")).hexdigest()


def retained_mcp_transcript(transcript: list[dict]) -> dict:
    entries = []
    for item in transcript:
        request = item.get("request") if isinstance(item, dict) else None
        response = item.get("response") if isinstance(item, dict) else None
        require(
            isinstance(request, dict) and isinstance(response, dict),
            "MCP transcript entry is malformed",
        )
        entries.append(
            {
                "request_id_sha256": canonical_sha256(request.get("id")),
                "method": require_nonempty_string(
                    request.get("method"), "MCP transcript method"
                ),
                "response_status": "error" if "error" in response else "ok",
            }
        )
    return {
        "schema_version": 1,
        "entry_count": len(entries),
        "raw_transcript_sha256": canonical_sha256(transcript),
        "entries": entries,
    }


def retained_runtime_evidence(runtime: dict) -> dict:
    return json.loads(
        json.dumps(
            {key: value for key, value in runtime.items() if not key.startswith("_")}
        )
    )


def _assert_private_fields(
    value: object,
    artifact_name: str,
    field_path: str = "$",
) -> None:
    if isinstance(value, dict):
        for key, child in value.items():
            require(
                isinstance(key, str),
                f"retained evidence {artifact_name} has a non-string field",
            )
            normalized = key.lower()
            require(
                normalized
                not in {
                    "directory",
                    "output_directory",
                    "project_path",
                    "project_root",
                    "repository_path",
                    "request_text",
                    "query_text",
                    "question_text",
                    "qualification_nonce",
                },
                f"retained evidence {artifact_name} leaked private field {field_path}.{key}",
            )
            _assert_private_fields(child, artifact_name, f"{field_path}.{key}")
    elif isinstance(value, list):
        for index, child in enumerate(value):
            _assert_private_fields(child, artifact_name, f"{field_path}[{index}]")
    elif isinstance(value, str):
        require(
            not Path(value).is_absolute(),
            f"retained evidence {artifact_name} leaked an absolute path at {field_path}",
        )


def assert_retained_json_privacy(target: Path, forbidden_values: list[str]) -> None:
    forbidden = sorted(
        {value for value in forbidden_values if isinstance(value, str) and value}
    )
    paths = [target] if target.is_file() else sorted(target.rglob("*.json"))
    for path in paths:
        require(
            path.is_file() and not path.is_symlink(), f"retained JSON is unsafe: {path}"
        )
        payload = path.read_bytes()
        for value in forbidden:
            require(
                value.encode("utf-8") not in payload,
                f"retained evidence {path.name} leaked private runtime material",
            )
        try:
            document = json.loads(payload)
        except json.JSONDecodeError as exc:
            raise ProofFailure(
                f"retained evidence {path.name} is not valid JSON: {exc}"
            ) from exc

        _assert_private_fields(document, path.name)


def require_sha256(value: object, field: str) -> str:
    require(
        isinstance(value, str)
        and HEX_SHA256.fullmatch(value) is not None
        and value != "0" * 64,
        f"{field} must be a lowercase SHA-256 digest",
    )
    return value


def require_nonempty_string(value: object, field: str) -> str:
    require(
        isinstance(value, str) and bool(value.strip()),
        f"{field} must be a non-empty string",
    )
    return value


def require_nonnegative_int(value: object, field: str) -> int:
    require(
        isinstance(value, int) and not isinstance(value, bool) and value >= 0,
        f"{field} must be a non-negative integer",
    )
    return value


def require_positive_int(value: object, field: str) -> int:
    value = require_nonnegative_int(value, field)
    require(value > 0, f"{field} must be positive")
    return value


def require_exact_keys(value: dict, expected: set[str], field: str) -> None:
    actual = set(value)
    require(
        actual == expected,
        f"{field} fields differ from the contract; missing={sorted(expected - actual)}, "
        f"unknown={sorted(actual - expected)}",
    )


def write_private_json(path: Path, value: object) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.parent.chmod(0o700)
    temporary = path.parent / f".{path.name}.{os.getpid()}.{secrets.token_hex(8)}.tmp"
    descriptor = os.open(temporary, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
    try:
        with os.fdopen(descriptor, "w", encoding="utf-8") as handle:
            json.dump(value, handle, sort_keys=True, separators=(",", ":"))
            handle.write("\n")
            handle.flush()
            os.fsync(handle.fileno())
        os.replace(temporary, path)
        path.chmod(0o600)
    except BaseException:
        try:
            temporary.unlink()
        except FileNotFoundError:
            pass
        raise


def require_opaque_identifier(value: object, field: str, *, length: int = 128) -> str:
    require(
        isinstance(value, str)
        and 1 <= len(value) <= length
        and re.fullmatch(r"[A-Za-z0-9._:-]+", value) is not None,
        f"{field} must be an opaque identifier without path or request text",
    )
    return value


def validate_runtime_claim_scope(args: argparse.Namespace) -> None:
    if args.server_behavior_only:
        require(
            not args.version_only and args.proof_tier != "calibration",
            "server-behavior-only proof requires a frozen non-calibration runtime tier",
        )
        require(
            not args.produce_qualification_evidence
            and args.qualification_evidence is None
            and args.retrieval_quality_evidence is None
            and args.publication_fault_evidence is None,
            "server-behavior-only proof rejects qualification and retrieval-quality inputs",
        )
    if args.ground_only:
        require(
            not args.version_only and args.plugin_handoff and args.project is not None,
            "ground-only proof requires plugin handoff and one project",
        )
        require(
            not args.server_behavior_only
            and not args.produce_qualification_evidence
            and args.qualification_evidence is None
            and args.retrieval_quality_evidence is None
            and args.publication_fault_evidence is None,
            "ground-only proof rejects server, qualification, and retrieval-quality inputs",
        )
