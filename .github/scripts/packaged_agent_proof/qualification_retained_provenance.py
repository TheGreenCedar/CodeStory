"""Retained package, installation, host, and runtime provenance verification."""

from __future__ import annotations

import re
from pathlib import Path

from .contract_primitives import (
    normalized_backend,
    require_nonempty_string,
    require_positive_int,
    require_sha256,
)
from .foundation import (
    CANDIDATE_PRODUCER_WORKFLOW_PATHS,
    PINNED_CODEX_CLI_VERSION,
    require,
)
from .qualification_retained_types import (
    RetainedPackageBinding,
    RetainedQualificationContract,
    RetainedRuntimeBinding,
)


def _verify_marketplace_provenance(
    contract: RetainedQualificationContract,
    plugin: dict,
    runtime: dict,
) -> None:
    require(
        plugin.get("marketplace_repository") == "TheGreenCedar/AgentPluginMarketplace"
        and plugin.get("codex_cli_version") == PINNED_CODEX_CLI_VERSION
        and runtime.get("build_source") == "github_release"
        and runtime.get("repo_ref") == f"v{contract.manifest['release_version']}",
        "installed evidence has invalid marketplace/release provenance",
    )
    require(
        isinstance(plugin.get("marketplace_commit"), str)
        and re.fullmatch(r"[0-9a-f]{40}", plugin["marketplace_commit"]) is not None,
        "installed evidence marketplace commit is invalid",
    )


def _verify_candidate_provenance(
    contract: RetainedQualificationContract,
    plugin: dict,
    runtime: dict,
) -> None:
    manifest = contract.manifest
    producer = plugin.get("producer")
    require(
        plugin.get("candidate_archive_sha256") == contract.archive_sha256
        and plugin.get("candidate_asset_target") == manifest["asset_target"]
        and plugin.get("plugin_source_tree") == manifest["source"]["tree"]
        and runtime.get("build_source") == "candidate_archive"
        and runtime.get("repo_ref") == manifest["source"]["commit"],
        "installed evidence has invalid staged-candidate provenance",
    )
    require(
        isinstance(producer, dict)
        and producer.get("repository") == "TheGreenCedar/CodeStory"
        and producer.get("workflow_path") in CANDIDATE_PRODUCER_WORKFLOW_PATHS
        and isinstance(producer.get("run_id"), str)
        and re.fullmatch(r"[1-9][0-9]*", producer["run_id"]) is not None
        and isinstance(producer.get("run_attempt"), str)
        and re.fullmatch(r"[1-9][0-9]*", producer["run_attempt"]) is not None,
        "installed evidence has unauthenticated candidate producer identity",
    )


def _verify_installed_provenance(contract: RetainedQualificationContract) -> None:
    retained = contract.evidence
    if retained.tier != "installed_runtime":
        return
    plugin = retained.installed_plugin
    runtime = retained.managed_runtime
    assert plugin is not None and runtime is not None
    manifest = contract.manifest
    installation_source = plugin.get("installation_source")
    require(
        plugin.get("schema_version") == 2
        and installation_source in {"codex_marketplace_install", "candidate_archive"}
        and plugin.get("plugin_id") == "codestory"
        and plugin.get("plugin_version") == manifest["release_version"],
        "installed evidence has invalid plugin provenance",
    )
    if installation_source == "codex_marketplace_install":
        _verify_marketplace_provenance(contract, plugin, runtime)
    else:
        _verify_candidate_provenance(contract, plugin, runtime)
    require_sha256(
        plugin.get("plugin_package_sha256"),
        "installed evidence plugin_package_sha256",
    )
    require(
        plugin.get("plugin_source_commit") == manifest["source"]["commit"],
        "installed evidence does not bind the marketplace plugin to the packaged source commit",
    )
    require(
        runtime.get("cli_source") == "managed"
        and runtime.get("plugin_version") == manifest["release_version"]
        and runtime.get("managed_binary_sha256") == manifest["binary"]["sha256"]
        and runtime.get("archive_sha256") == contract.archive_sha256,
        "installed evidence does not bind the exact managed runtime",
    )
    if contract.live_installed_plugin is not None:
        require(
            plugin == contract.live_installed_plugin,
            "retained installed plugin provenance is stale",
        )
    if contract.live_managed_runtime is not None:
        require(
            runtime == contract.live_managed_runtime,
            "retained managed runtime provenance is stale",
        )


def _verify_source_and_package(
    contract: RetainedQualificationContract,
) -> RetainedPackageBinding:
    retained = contract.evidence
    manifest = contract.manifest
    package = retained.package
    require(
        retained.source == manifest["source"],
        "retained qualification source identity does not match package",
    )
    package_fields = (
        ("archive_sha256", contract.archive_sha256, "archive"),
        ("executable_sha256", manifest["binary"]["sha256"], "executable"),
        ("asset_target", manifest["asset_target"], "package target"),
        ("release_version", manifest["release_version"], "release version"),
        ("model_sha256", manifest["model"]["sha256"], "model"),
    )
    for field, expected, description in package_fields:
        require(
            package.get(field) == expected,
            f"retained qualification names a different {description}",
        )
    require(
        package.get("matrix_cell_id") == contract.matrix_cell_id
        and package.get("policy") == contract.expected_policy
        and normalized_backend(package.get("backend"))
        == normalized_backend(contract.expected_backend)
        and package.get("accelerator_claim") == contract.accelerator_claim,
        "retained qualification package does not match the requested matrix cell, policy, backend, or accelerator claim",
    )
    for field in (
        "protocol_sha256",
        "constant_set_sha256",
        "measurement_protocol_sha256",
    ):
        require(
            package.get(field) == manifest["server_proof"][field],
            f"retained qualification {field} does not match package",
        )
    return RetainedPackageBinding(retained.source, package, contract.matrix_cell)


def _verify_host(contract: RetainedQualificationContract) -> None:
    retained = contract.evidence
    host = retained.host
    package = retained.package
    require_sha256(host.get("fingerprint"), "retained qualification host fingerprint")
    require_nonempty_string(
        host.get("platform"), "retained qualification host platform"
    )
    require(
        host.get("target") == contract.manifest["asset_target"],
        "retained qualification host names a different package target",
    )
    require_nonempty_string(host.get("backend"), "retained qualification host backend")
    require(
        host.get("matrix_cell_id") == contract.matrix_cell_id
        and host.get("accelerator_claim") == contract.accelerator_claim
        and host.get("host_class") == contract.matrix_cell["host_class"],
        "retained qualification host does not match the requested matrix cell",
    )
    require(
        normalized_backend(package.get("backend"))
        == normalized_backend(host["backend"]),
        "retained qualification package and host backend identities disagree",
    )
    require(
        package.get("policy") == host.get("policy")
        and host.get("policy") in {"accelerated", "cpu_explicit"},
        "retained qualification package and host policy identities disagree",
    )
    require(
        host.get("policy") == contract.expected_policy
        and normalized_backend(host.get("backend"))
        == normalized_backend(contract.expected_backend),
        "retained qualification host used the wrong requested policy or backend",
    )
    for field in ("cache_state", "residency_state"):
        require_nonempty_string(host.get(field), f"retained qualification host {field}")
        require(
            package.get(field) == host[field],
            f"retained qualification package and host {field} disagree",
        )
        require(
            host[field] == contract.matrix_cell[field],
            f"retained qualification host {field} differs from the selected matrix cell",
        )
    require(
        host.get("unplanned_suspend") is False,
        "retained qualification host recorded an unplanned suspend",
    )


def _verify_same_account(contract: RetainedQualificationContract) -> None:
    same_account = contract.evidence.same_account
    require_nonempty_string(same_account.get("account_id"), "same_account.account_id")
    require(
        same_account.get("relation") == "same_os_account",
        "retained qualification does not prove same-OS-account scope",
    )
    hosts = same_account.get("plugin_hosts")
    require(
        isinstance(hosts, list) and len(hosts) == 2,
        "qualification requires exactly two plugin hosts",
    )
    host_ids: set[tuple[object, object]] = set()
    repository_ids: set[str] = set()
    for index, host in enumerate(hosts):
        require(isinstance(host, dict), f"plugin host {index} is malformed")
        require_positive_int(host.get("pid"), f"plugin_hosts[{index}].pid")
        start_id = require_nonempty_string(
            host.get("process_start_id"),
            f"plugin_hosts[{index}].process_start_id",
        )
        repository_id = require_nonempty_string(
            host.get("repository_id"),
            f"plugin_hosts[{index}].repository_id",
        )
        require(
            not Path(repository_id).is_absolute(),
            "retained plugin-host evidence must use an opaque repository identity, not a path",
        )
        host_ids.add((host["pid"], start_id))
        repository_ids.add(repository_id)
    require(len(host_ids) == 2, "plugin hosts are not independently started processes")
    require(len(repository_ids) == 2, "plugin hosts did not use different repositories")
    require(
        same_account.get("cross_login_or_terminal_sessions_proven") is False,
        "base same-account evidence must not infer cross-session sharing",
    )


def _verify_shared_identity_and_timing(
    contract: RetainedQualificationContract,
) -> None:
    retained_shared = contract.evidence.shared_identity
    live_shared = contract.live_shared_identity
    require(
        isinstance(live_shared, dict),
        "live two-host proof omitted shared server identity",
    )
    identity_fields = (
        "endpoint_namespace_id",
        "lifetime_authority_id",
        "listener_id",
        "server_instance_id",
        "server_process_start_id",
        "engine_owner_id",
        "native_worker_id",
        "load_generation",
        "model_load_count",
    )
    for field in identity_fields:
        require(field in retained_shared, f"retained shared identity omitted {field}")
        require(
            retained_shared[field] == live_shared[field],
            f"retained shared identity {field} does not match the live two-host proof",
        )
    require(
        retained_shared["model_load_count"] == 1,
        "retained cold race did not prove one model load",
    )
    timing = contract.evidence.timing
    require(
        timing.get("clock_domain") == "awake_monotonic",
        "qualification used the wrong clock domain",
    )
    require(
        timing.get("cross_process_timestamp_subtraction") is False,
        "qualification subtracted cross-process timestamps",
    )
    require(
        timing.get("unplanned_suspend") is False,
        "qualification performance block included suspend",
    )
    require(
        timing.get("constants_frozen_before_run") is True,
        "qualification selected constants from its own results",
    )
    require(
        timing.get("constant_set_sha256")
        == contract.manifest["server_proof"]["constant_set_sha256"],
        "qualification timing used a different constant set",
    )


def _verify_runtime_binding(
    contract: RetainedQualificationContract,
) -> RetainedRuntimeBinding:
    _verify_installed_provenance(contract)
    _verify_host(contract)
    _verify_same_account(contract)
    _verify_shared_identity_and_timing(contract)
    retained = contract.evidence
    return RetainedRuntimeBinding(
        retained.installed_plugin,
        retained.managed_runtime,
        retained.host,
        retained.same_account,
        retained.shared_identity,
        retained.timing,
    )
