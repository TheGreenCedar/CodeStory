"""Retained five-process memory evidence."""

from __future__ import annotations

import hashlib
from pathlib import Path

from .contract_primitives import (
    require_exact_keys,
    require_nonempty_string,
    require_opaque_identifier,
    require_positive_int,
    write_private_json,
)
from .foundation import MEMORY_EVIDENCE_CONTRACT, TARGET_CONTRACTS, require
from .measurement_samples import (
    qualification_measurement_sample_value,
    selected_qualification_matrix_cell,
)


def _write_memory_payload(
    artifact_root: Path,
    raw: object,
    *,
    source: dict,
    package: dict,
    contracts: dict,
    forbidden_values: list[str],
) -> tuple[str, dict, bytes]:
    require(
        isinstance(raw, dict), "live runtime omitted five-process memory observations"
    )
    require_exact_keys(
        raw,
        {"evidence_contract", "metric", "unit", "samples"},
        "five-process memory observations",
    )
    payload = {
        "schema_version": 1,
        "source": source,
        "package": package,
        "contracts": contracts,
        **raw,
    }
    name = "total-codestory-process-memory.raw.json"
    path = artifact_root / name
    write_private_json(path, payload)
    payload_bytes = path.read_bytes()
    for forbidden in forbidden_values:
        require(
            forbidden.encode("utf-8") not in payload_bytes,
            "five-process memory artifact leaked private request material",
        )
    require_exact_keys(
        payload,
        {
            "schema_version",
            "source",
            "package",
            "contracts",
            "evidence_contract",
            "metric",
            "unit",
            "samples",
        },
        "five-process memory artifact",
    )
    require(
        payload["schema_version"] == 1
        and payload["source"] == source
        and payload["package"] == package
        and payload["contracts"] == contracts
        and payload["evidence_contract"] == MEMORY_EVIDENCE_CONTRACT
        and payload["metric"] == "total_codestory_process_memory"
        and payload["unit"] == "bytes",
        "five-process memory artifact changed its bound contract",
    )
    return name, payload, payload_bytes


def _memory_sample_identity(
    sample: dict,
    *,
    index: int,
    sample_ids: set[str],
) -> tuple[str, str, int]:
    sample_id = require_opaque_identifier(
        sample["sample_id"],
        f"five-process memory sample {index}.sample_id",
    )
    require(sample_id not in sample_ids, "five-process memory sample id was reused")
    sample_ids.add(sample_id)
    server = sample["server_identity"]
    require(
        isinstance(server, dict),
        f"five-process memory sample {index} server identity is malformed",
    )
    require_exact_keys(
        server,
        {"server_instance_id", "process_start_id", "load_generation"},
        f"five-process memory sample {index} server identity",
    )
    return (
        require_opaque_identifier(
            server["server_instance_id"],
            f"five-process memory sample {index}.server_instance_id",
        ),
        require_nonempty_string(
            server["process_start_id"],
            f"five-process memory sample {index}.server_process_start_id",
        ),
        require_positive_int(
            server["load_generation"],
            f"five-process memory sample {index}.load_generation",
        ),
    )


def _verify_memory_sample(
    sample: object,
    *,
    index: int,
    matrix_cell_id: str,
    matrix_cell: dict,
    protocol: dict,
    package: dict,
    expected_measurement_api: str,
    sample_ids: set[str],
) -> tuple[dict, tuple[str, str, int]]:
    require(
        isinstance(sample, dict), f"five-process memory sample {index} is malformed"
    )
    require_exact_keys(
        sample,
        {
            "sample_id",
            "repeat",
            "matrix_cell_id",
            "workload_id",
            "cache_state",
            "residency_state",
            "producer_process",
            "server_identity",
            "clock",
            "start",
            "end",
            "operands",
            "suspend_witness",
        },
        f"five-process memory sample {index}",
    )
    require(
        sample["repeat"] == index + 1
        and sample["matrix_cell_id"] == matrix_cell_id
        and sample["workload_id"]
        == protocol["workloads"]["total_codestory_process_memory"]["workload_id"]
        and sample["cache_state"] == matrix_cell["cache_state"]
        and sample["residency_state"] == matrix_cell["residency_state"],
        "five-process memory sample changed its preregistered cell or workload",
    )
    processes = sample.get("operands", {}).get("processes", [])
    require(
        all(
            isinstance(process, dict)
            and process.get("measurement_api") == expected_measurement_api
            for process in processes
        ),
        "five-process memory sample used the wrong platform memory API",
    )
    package_roles = {"plugin_cli_a", "plugin_cli_b", "embedding_server"}
    require(
        all(
            process.get("executable_sha256") == package["executable_sha256"]
            for process in processes
            if process.get("role") in package_roles
        ),
        "five-process memory sample used a different packaged executable",
    )
    identity = _memory_sample_identity(sample, index=index, sample_ids=sample_ids)
    adapted = {**sample, "process": sample["producer_process"]}
    adapted.pop("producer_process")
    return adapted, identity


def retain_five_process_memory_evidence(
    artifact_root: Path,
    raw: object,
    *,
    source: dict,
    package: dict,
    contracts: dict,
    protocol: dict,
    target: str,
    proof_tier: str,
    matrix_cell_id: str,
    expected_policy: str,
    expected_backend: str,
    forbidden_values: list[str],
) -> dict:
    name, payload, payload_bytes = _write_memory_payload(
        artifact_root,
        raw,
        source=source,
        package=package,
        contracts=contracts,
        forbidden_values=forbidden_values,
    )
    matrix_cell = selected_qualification_matrix_cell(
        protocol,
        cell_id=matrix_cell_id,
        target=target,
        proof_tier=proof_tier,
        expected_policy=expected_policy,
        expected_backend=expected_backend,
    )
    samples = payload["samples"]
    require(
        isinstance(samples, list) and len(samples) == 3,
        "five-process memory evidence requires three samples",
    )
    target_os = TARGET_CONTRACTS[target]["target_os"]
    clock_policy = protocol["clock_policy"]
    suspend_contract = clock_policy["suspend_detection"]
    expected_measurement_api = {
        "linux": "rss",
        "macos": "macos_physical_footprint",
        "windows": "windows_working_set",
    }[target_os]
    values = []
    sample_ids: set[str] = set()
    server_identities: set[tuple[str, str, int]] = set()
    for index, sample in enumerate(samples):
        adapted, identity = _verify_memory_sample(
            sample,
            index=index,
            matrix_cell_id=matrix_cell_id,
            matrix_cell=matrix_cell,
            protocol=protocol,
            package=package,
            expected_measurement_api=expected_measurement_api,
            sample_ids=sample_ids,
        )
        server_identities.add(identity)
        values.append(
            qualification_measurement_sample_value(
                "total_codestory_process_memory",
                adapted,
                contracts=contracts,
                phase_boundaries=protocol["phase_boundaries"],
                allowed_awake_apis=set(clock_policy["platform_apis"][target_os]),
                inclusive_api=suspend_contract["platform_apis"][target_os],
                maximum_suspend_ns=suspend_contract["maximum_inclusive_minus_awake_ns"],
                expected_policy=expected_policy,
                expected_backend=expected_backend,
            )
        )
    require(
        len(server_identities) == 1,
        "five-process memory block changed shared server identity",
    )
    return {
        "artifact": {
            "name": name,
            "sha256": hashlib.sha256(payload_bytes).hexdigest(),
        },
        "value": max(values),
        "payload": payload,
    }
