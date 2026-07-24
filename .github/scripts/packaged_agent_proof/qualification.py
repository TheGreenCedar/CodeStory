"""Qualification for packaged CodeStory proof."""

from __future__ import annotations

import hashlib
import json
import re
from collections import Counter
from dataclasses import dataclass
from pathlib import Path

from .foundation import (
    CANDIDATE_PRODUCER_WORKFLOW_PATHS,
    CANDIDATE_QUALIFICATION_MATRIX_ALIASES,
    EXTERNAL_QUALIFICATION_METRICS,
    HEX_SHA256,
    LOWER_TIER_NONCLAIMS,
    MIN_RETRIEVAL_QUALITY_REPEATS,
    PINNED_CODEX_CLI_VERSION,
    QUALIFICATION_SCHEMA_VERSION,
    RELEASE_QUALITY_CORPUS_ID,
    REQUIRED_SERVER_SCENARIOS,
    RETRIEVAL_QUALITY_EVIDENCE_CONTRACT,
    SERVER_LIFECYCLES,
    TARGET_CONTRACTS,
    ProofFailure,
    require,
)
from .contracts import (
    qualification_measurement_sample_value,
    require_exact_keys,
    require_nonempty_string,
    require_nonnegative_int,
    require_opaque_identifier,
    require_positive_int,
    require_sha256,
    selected_qualification_matrix_cell,
)
from .archive import (
    normalized_backend,
)

@dataclass(frozen=True)
class RetainedQualificationEvidence:
    raw: dict
    tier: str
    installed_plugin: dict | None
    managed_runtime: dict | None
    source: dict
    package: dict
    host: dict
    same_account: dict
    shared_identity: dict
    timing: dict
    scenarios: dict
    lower_tier_nonclaims: dict
    metrics: dict

    @classmethod
    def from_dict(cls, evidence: dict) -> RetainedQualificationEvidence:
        require(
            evidence.get("schema_version") == QUALIFICATION_SCHEMA_VERSION,
            "retained qualification schema is unsupported",
        )
        require(
            evidence.get("status") == "pass",
            "retained qualification is not a passing result",
        )
        tier = evidence.get("tier")
        require(
            tier in {"hosted_package", "protected_hardware", "installed_runtime"},
            "retained qualification tier is invalid",
        )
        installed_plugin = evidence.get("installed_plugin")
        managed_runtime = evidence.get("managed_runtime")
        if tier == "installed_runtime":
            require(
                isinstance(installed_plugin, dict),
                "installed evidence omitted plugin provenance",
            )
            require(
                isinstance(managed_runtime, dict),
                "installed evidence omitted managed runtime provenance",
            )
        else:
            require(
                installed_plugin is None and managed_runtime is None,
                "lower-tier evidence must not claim installed plugin provenance",
            )
        fields = {
            "source": "retained qualification omitted source identity",
            "package": "retained qualification omitted package identity",
            "host": "retained qualification omitted host identity",
            "same_account": "retained qualification omitted same-account evidence",
            "shared_identity": "retained qualification omitted shared server identity",
            "timing": "retained qualification omitted timing identity",
            "scenarios": "retained qualification omitted scenario evidence",
            "lower_tier_nonclaims": "retained qualification omitted lower-tier nonclaims",
            "metrics": "retained qualification omitted metric results",
        }
        normalized = {}
        for field, message in fields.items():
            value = evidence.get(field)
            require(isinstance(value, dict), message)
            normalized[field] = value
        return cls(
            evidence,
            tier,
            installed_plugin,
            managed_runtime,
            **normalized,
        )


@dataclass(frozen=True)
class RetainedQualificationContract:
    evidence: RetainedQualificationEvidence
    manifest: dict
    archive_sha256: str
    live_shared_identity: dict
    measurement_contract: dict
    required_tier: str
    matrix_cell_id: str
    expected_policy: str
    expected_backend: str
    accelerator_claim: str
    matrix_cell: dict
    live_installed_plugin: dict | None
    live_managed_runtime: dict | None

    @classmethod
    def create(
        cls,
        evidence: dict,
        *,
        manifest: dict,
        archive_sha256: str,
        shared_identity: dict,
        measurement_contract: dict,
        required_tier: str,
        required_matrix_cell_id: str,
        expected_policy: str,
        expected_backend: str,
        expected_accelerator_claim: str,
        installed_plugin: dict | None,
        managed_runtime: dict | None,
    ) -> RetainedQualificationContract:
        retained = RetainedQualificationEvidence.from_dict(evidence)
        require(
            retained.tier == required_tier,
            f"retained {retained.tier} evidence cannot support exact requested tier {required_tier}",
        )
        matrix_cell = selected_qualification_matrix_cell(
            measurement_contract["measurement_protocol"],
            cell_id=required_matrix_cell_id,
            target=manifest["asset_target"],
            proof_tier=required_tier,
            expected_policy=expected_policy,
            expected_backend=expected_backend,
        )
        require(
            matrix_cell["accelerator_claim"] == expected_accelerator_claim,
            "requested accelerator claim does not match the selected qualification matrix cell",
        )
        return cls(
            retained,
            manifest,
            archive_sha256,
            shared_identity,
            measurement_contract,
            required_tier,
            required_matrix_cell_id,
            expected_policy,
            expected_backend,
            expected_accelerator_claim,
            matrix_cell,
            installed_plugin,
            managed_runtime,
        )


@dataclass(frozen=True)
class RetainedPackageBinding:
    source: dict
    package: dict
    matrix_cell: dict


@dataclass(frozen=True)
class RetainedRuntimeBinding:
    installed_plugin: dict | None
    managed_runtime: dict | None
    host: dict
    same_account: dict
    shared_identity: dict
    timing: dict


@dataclass(frozen=True)
class RetainedMeasurementBinding:
    scenarios: dict
    lower_tier_nonclaims: dict
    metrics: tuple[RetainedMetric, ...]


@dataclass(frozen=True)
class VerifiedRetainedQualification:
    parsed: RetainedQualificationEvidence
    package: RetainedPackageBinding
    runtime: RetainedRuntimeBinding
    measurements: RetainedMeasurementBinding


def _verify_marketplace_provenance(
    contract: RetainedQualificationContract,
    plugin: dict,
    runtime: dict,
) -> None:
    require(
        plugin.get("marketplace_repository")
        == "TheGreenCedar/AgentPluginMarketplace"
        and plugin.get("codex_cli_version") == PINNED_CODEX_CLI_VERSION
        and runtime.get("build_source") == "github_release"
        and runtime.get("repo_ref")
        == f"v{contract.manifest['release_version']}",
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
        and installation_source
        in {"codex_marketplace_install", "candidate_archive"}
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
    require_nonempty_string(host.get("platform"), "retained qualification host platform")
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
        normalized_backend(package.get("backend")) == normalized_backend(host["backend"]),
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


def _verified_scenario_artifact_names(scenario_id: str, artifacts: object) -> set[str]:
    require(
        isinstance(artifacts, list) and artifacts,
        f"scenario {scenario_id} has no retained artifacts",
    )
    names = set()
    for artifact in artifacts:
        require(isinstance(artifact, dict), f"scenario {scenario_id} artifact is malformed")
        name = require_nonempty_string(
            artifact.get("name"),
            f"scenario {scenario_id} artifact name",
        )
        require(
            Path(name).name == name and Path(name).suffix == ".json",
            f"scenario {scenario_id} artifact name is not a safe JSON basename",
        )
        require_sha256(artifact.get("sha256"), f"scenario {scenario_id} artifact sha256")
        names.add(name)
    return names


def _verify_scenarios(contract: RetainedQualificationContract) -> None:
    scenarios = contract.evidence.scenarios
    scenario_contracts = contract.measurement_contract["measurement_protocol"][
        "scenario_contracts"
    ]
    require(
        set(scenarios) == REQUIRED_SERVER_SCENARIOS,
        "retained qualification scenario set is incomplete",
    )
    for scenario_id in sorted(REQUIRED_SERVER_SCENARIOS):
        scenario = scenarios.get(scenario_id)
        require(isinstance(scenario, dict), f"scenario {scenario_id} is malformed")
        require(scenario.get("status") == "pass", f"scenario {scenario_id} did not pass")
        assertions = scenario.get("assertions")
        require(
            isinstance(assertions, dict),
            f"scenario {scenario_id} omitted assertions",
        )
        required_assertions = set(scenario_contracts[scenario_id]["required"])
        require(
            set(assertions) == required_assertions,
            f"scenario {scenario_id} assertions do not match the preregistered contract",
        )
        failed = sorted(name for name, passed in assertions.items() if passed is not True)
        require(
            not failed,
            f"scenario {scenario_id} has failed assertions: " + ", ".join(failed),
        )
        artifact_names = _verified_scenario_artifact_names(
            scenario_id,
            scenario.get("artifacts"),
        )
        if scenario_id in {"server_crash", "worker_stall"}:
            require(
                "publication-fault-external.raw.json" in artifact_names,
                f"{scenario_id} scenario omitted separately hashed publication-fence evidence",
            )


def _verify_nonclaims(contract: RetainedQualificationContract) -> None:
    nonclaims = contract.evidence.lower_tier_nonclaims
    require(
        set(nonclaims) == LOWER_TIER_NONCLAIMS,
        "retained qualification nonclaim set is incomplete",
    )
    for claim, record in nonclaims.items():
        require(isinstance(record, dict), f"nonclaim {claim} is malformed")
        require(
            record.get("claimed") is False,
            f"lower-tier evidence incorrectly claims {claim}",
        )
        require_nonempty_string(record.get("reason"), f"nonclaim {claim} reason")


@dataclass(frozen=True)
class RetainedMetric:
    name: str
    value: int | float
    threshold: int | float
    comparison: str
    raw_evidence: dict


def _normalized_retained_metrics(
    contract: RetainedQualificationContract,
) -> tuple[RetainedMetric, ...]:
    metrics = contract.evidence.metrics
    protocol = contract.measurement_contract["measurement_protocol"]
    required_metrics = set(protocol["required_metrics"])
    require(
        set(metrics) == required_metrics,
        "retained qualification metric set is incomplete",
    )
    thresholds = contract.measurement_contract["constant_set"][
        "qualification_thresholds"
    ]
    metric_contracts = protocol["metric_contracts"]
    normalized = []
    for metric, result in metrics.items():
        require(isinstance(result, dict), f"metric {metric} is malformed")
        require(
            result.get("status") == "pass",
            f"metric {metric} did not pass its frozen threshold",
        )
        require(
            result.get("unit") == metric_contracts[metric]["unit"],
            f"metric {metric} used the wrong unit",
        )
        value = result.get("value")
        require(
            isinstance(value, (int, float)) and not isinstance(value, bool),
            f"metric {metric} value is not numeric",
        )
        threshold = result.get("threshold")
        require(
            threshold == thresholds[metric]
            and isinstance(threshold, (int, float))
            and not isinstance(threshold, bool),
            f"metric {metric} threshold does not match the frozen constant set",
        )
        comparison = metric_contracts[metric]["comparison"]
        require(
            result.get("comparison") == comparison,
            f"metric {metric} used the wrong comparison",
        )
        raw_evidence = result.get("raw_evidence")
        require(
            isinstance(raw_evidence, dict),
            (
                "retrieval quality metric omitted raw evidence"
                if metric == "retrieval_quality"
                else f"metric {metric} omitted its raw measurement artifact"
            ),
        )
        normalized.append(
            RetainedMetric(metric, value, threshold, comparison, raw_evidence)
        )
    return tuple(normalized)


def _verify_retrieval_quality_metric(
    contract: RetainedQualificationContract,
    metric: RetainedMetric,
) -> None:
    raw_evidence = metric.raw_evidence
    require_exact_keys(
        raw_evidence,
        {
            "artifact",
            "evaluation_contract",
            "source_commit",
            "source_tree",
            "corpus_id",
            "holdout_manifest_set_sha256",
            "repeats",
            "row_count",
            "passing_row_count",
            "publishable_packet_pass_rate",
        },
        "retrieval quality retained raw evidence",
    )
    artifact = raw_evidence["artifact"]
    require(isinstance(artifact, dict), "retrieval quality raw artifact is malformed")
    require_exact_keys(artifact, {"name", "sha256"}, "retrieval quality raw artifact")
    require(
        artifact["name"] == "packet-runtime-summary.json",
        "retrieval quality raw artifact name is invalid",
    )
    require_sha256(artifact["sha256"], "retrieval quality raw artifact sha256")
    require(
        raw_evidence["evaluation_contract"] == RETRIEVAL_QUALITY_EVIDENCE_CONTRACT,
        "retrieval quality retained evaluation contract changed",
    )
    require(
        raw_evidence["source_commit"] == contract.evidence.source["commit"]
        and raw_evidence["source_tree"] == contract.evidence.source["tree"],
        "retrieval quality retained source identity is stale",
    )
    require(
        require_positive_int(raw_evidence["repeats"], "retrieval quality repeats")
        == MIN_RETRIEVAL_QUALITY_REPEATS,
        "retrieval quality retained the wrong repeat count",
    )
    require(
        raw_evidence["corpus_id"] == RELEASE_QUALITY_CORPUS_ID,
        "retrieval quality retained the wrong holdout corpus",
    )
    require_sha256(
        raw_evidence["holdout_manifest_set_sha256"],
        "retrieval quality holdout manifest set sha256",
    )
    row_count = require_positive_int(
        raw_evidence["row_count"],
        "retrieval quality row count",
    )
    require(
        require_positive_int(
            raw_evidence["passing_row_count"],
            "retrieval quality passing row count",
        )
        == row_count,
        "retrieval quality retained a failing row",
    )
    pass_rate = raw_evidence["publishable_packet_pass_rate"]
    require(
        isinstance(pass_rate, (int, float)) and not isinstance(pass_rate, bool),
        "retrieval quality pass rate is not numeric",
    )
    require(
        pass_rate == metric.value,
        "retrieval quality metric does not match its raw evidence",
    )


def _verify_measurement_metric(metric: RetainedMetric) -> None:
    require_exact_keys(
        metric.raw_evidence,
        {"name", "sha256"},
        f"metric {metric.name} raw measurement artifact",
    )
    expected_artifact_name = (
        "total-codestory-process-memory.raw.json"
        if metric.name == "total_codestory_process_memory"
        else "measurements.raw.json"
    )
    require(
        metric.raw_evidence["name"] == expected_artifact_name,
        f"metric {metric.name} used the wrong raw measurement artifact",
    )
    require_sha256(
        metric.raw_evidence["sha256"],
        f"metric {metric.name} raw artifact sha256",
    )


def _verify_metrics(
    contract: RetainedQualificationContract,
) -> tuple[RetainedMetric, ...]:
    metrics = _normalized_retained_metrics(contract)
    for metric in metrics:
        if metric.name == "retrieval_quality":
            _verify_retrieval_quality_metric(contract, metric)
        else:
            _verify_measurement_metric(metric)
        passed = {
            "equal": metric.value == metric.threshold,
            "greater_than_or_equal": metric.value >= metric.threshold,
            "less_than_or_equal": metric.value <= metric.threshold,
        }[metric.comparison]
        require(
            passed,
            f"metric {metric.name} value failed its frozen comparison",
        )
    return metrics


def _verify_measurement_binding(
    contract: RetainedQualificationContract,
) -> RetainedMeasurementBinding:
    _verify_scenarios(contract)
    _verify_nonclaims(contract)
    metrics = _verify_metrics(contract)
    retained = contract.evidence
    return RetainedMeasurementBinding(
        retained.scenarios,
        retained.lower_tier_nonclaims,
        metrics,
    )


def verify_retained_qualification(
    evidence: dict,
    *,
    manifest: dict,
    archive_sha256: str,
    shared_identity: dict,
    measurement_contract: dict,
    required_tier: str,
    required_matrix_cell_id: str,
    expected_policy: str,
    expected_backend: str,
    expected_accelerator_claim: str,
    installed_plugin: dict | None = None,
    managed_runtime: dict | None = None,
) -> dict:
    contract = RetainedQualificationContract.create(
        evidence,
        manifest=manifest,
        archive_sha256=archive_sha256,
        shared_identity=shared_identity,
        measurement_contract=measurement_contract,
        required_tier=required_tier,
        required_matrix_cell_id=required_matrix_cell_id,
        expected_policy=expected_policy,
        expected_backend=expected_backend,
        expected_accelerator_claim=expected_accelerator_claim,
        installed_plugin=installed_plugin,
        managed_runtime=managed_runtime,
    )
    verified = VerifiedRetainedQualification(
        parsed=contract.evidence,
        package=_verify_source_and_package(contract),
        runtime=_verify_runtime_binding(contract),
        measurements=_verify_measurement_binding(contract),
    )
    return verified.parsed.raw


@dataclass(frozen=True)
class RetryState:
    code: str
    message_head: str
    retry_class: str
    retry_after_ms: int
    retry_condition: str
    capacity: object | None


def validate_retry_state(value: object, field: str) -> RetryState:
    require(isinstance(value, dict), f"{field} is malformed")
    expected = {
        "code",
        "message_head",
        "retry_class",
        "retry_after_ms",
        "retry_condition",
    }
    if "capacity" in value:
        expected.add("capacity")
    require_exact_keys(value, expected, field)
    code = require_nonempty_string(value["code"], f"{field}.code")
    message_head = require_nonempty_string(
        value["message_head"],
        f"{field}.message_head",
    )
    retry_class = require_nonempty_string(
        value["retry_class"],
        f"{field}.retry_class",
    )
    retry_after_ms = require_nonnegative_int(
        value["retry_after_ms"],
        f"{field}.retry_after_ms",
    )
    retry_condition = require_nonempty_string(
        value["retry_condition"],
        f"{field}.retry_condition",
    )
    require(
        retry_class
        in {
            "after_capacity_change",
            "after_delay",
            "after_owner_idle",
            "after_server_change",
            "none",
            "same_rpc_once",
            "terminal",
        },
        f"{field}.retry_class is outside the protocol contract",
    )
    return RetryState(
        code=code,
        message_head=message_head,
        retry_class=retry_class,
        retry_after_ms=retry_after_ms,
        retry_condition=retry_condition,
        capacity=value.get("capacity"),
    )


@dataclass(frozen=True)
class ReplayAttempt:
    ordinal: int
    request_id: str
    server_instance_id: str
    submitted_ns: int
    completed_ns: int
    outcome: str


def _validated_replay_attempt(value: object, index: int) -> ReplayAttempt:
    require(isinstance(value, dict), "replay attempt is malformed")
    require_exact_keys(
        value,
        {
            "ordinal",
            "request_id",
            "server_instance_id",
            "submitted_ns",
            "completed_ns",
            "outcome",
        },
        f"replay attempt {index}",
    )
    require(value["ordinal"] == index, "replay attempt ordinal is not exact")
    request_id = require_nonempty_string(
        value["request_id"],
        "replay attempt request ID",
    )
    server_instance_id = require_nonempty_string(
        value["server_instance_id"],
        f"replay attempt {index} server_instance_id",
    )
    submitted_ns = require_nonnegative_int(
        value["submitted_ns"],
        f"replay attempt {index} submitted_ns",
    )
    completed_ns = require_nonnegative_int(
        value["completed_ns"],
        f"replay attempt {index} completed_ns",
    )
    require(completed_ns >= submitted_ns, "replay attempt clock moved backwards")
    outcome = require_nonempty_string(
        value["outcome"],
        f"replay attempt {index} outcome",
    )
    return ReplayAttempt(
        ordinal=index,
        request_id=request_id,
        server_instance_id=server_instance_id,
        submitted_ns=submitted_ns,
        completed_ns=completed_ns,
        outcome=outcome,
    )


def validate_replay_attempts(
    values: dict,
    *,
    old_server_instance_id: str,
    new_server_instance_id: str,
) -> tuple[ReplayAttempt, ReplayAttempt]:
    raw_attempts = values["wire_attempts"]
    require(
        values["wire_attempt_count"] == 2
        and isinstance(raw_attempts, list)
        and len(raw_attempts) == 2,
        "replay evidence must contain exactly the original RPC and one replay",
    )
    attempts = (
        _validated_replay_attempt(raw_attempts[0], 1),
        _validated_replay_attempt(raw_attempts[1], 2),
    )
    original, replay = attempts
    require(
        original.request_id != replay.request_id
        and original.server_instance_id == old_server_instance_id
        and original.outcome == "server_loss"
        and replay.server_instance_id == new_server_instance_id
        and replay.outcome == "completed",
        "replay attempts do not bind the old loss and exact replacement completion",
    )
    return attempts


@dataclass(frozen=True)
class ScenarioAssertionEvidence:
    scenario_id: str
    observations_by_kind: dict[str, list[dict]]
    process_observations: list[dict]
    invocations: list[dict]
    same_account: dict
    materialization: dict
    snapshots: tuple[dict, ...]
    snapshot_instances: frozenset[str]
    snapshot_authorities: frozenset[tuple[str, str]]
    snapshot_engines: frozenset[tuple[str, str, int, int]]

    @classmethod
    def from_raw(
        cls,
        scenario_id: str,
        *,
        observations_by_kind: dict[str, list[dict]],
        process_observations: list[dict],
        invocations: list[dict],
        same_account: dict,
        materialization: dict,
    ) -> ScenarioAssertionEvidence:
        snapshots = tuple(
            observation["snapshot"]
            for observation in process_observations
            if observation.get("snapshot") is not None
        )
        return cls(
            scenario_id=scenario_id,
            observations_by_kind=observations_by_kind,
            process_observations=process_observations,
            invocations=invocations,
            same_account=same_account,
            materialization=materialization,
            snapshots=snapshots,
            snapshot_instances=frozenset(
                snapshot["process"]["server_instance_id"]
                for snapshot in snapshots
            ),
            snapshot_authorities=frozenset(
                (
                    snapshot["authority"]["lifetime_authority_id"],
                    snapshot["authority"]["listener_id"],
                )
                for snapshot in snapshots
            ),
            snapshot_engines=frozenset(
                (
                    snapshot["engine"]["engine_owner_id"],
                    snapshot["engine"]["native_worker_id"],
                    snapshot["engine"]["load_generation"],
                    snapshot["engine"]["model_load_count"],
                )
                for snapshot in snapshots
                if snapshot.get("engine") is not None
            ),
        )

    def transition(
        self,
        kind: str,
        expected_keys: set[str] | None = None,
    ) -> dict:
        matches = self.observations_by_kind.get(kind, [])
        require(
            len(matches) == 1,
            f"qualification scenario {self.scenario_id} omitted or duplicated transition {kind}",
        )
        values = matches[0]["values"]
        require(
            isinstance(values, dict),
            f"qualification transition {kind} values are malformed",
        )
        if expected_keys is not None:
            require_exact_keys(
                values,
                expected_keys,
                f"qualification transition {kind} values",
            )
        return values

    def scheduler(self, kind: str) -> dict:
        values = self.transition(
            kind,
            {
                "query_capacity",
                "query_depth",
                "bulk_capacity",
                "bulk_depth",
                "active_request_count",
                "lease_count",
                "active_request_class",
            },
        )
        for field in (
            "query_capacity",
            "query_depth",
            "bulk_capacity",
            "bulk_depth",
            "active_request_count",
            "lease_count",
        ):
            require_nonnegative_int(
                values[field],
                f"qualification transition {kind}.{field}",
            )
        require(
            values["active_request_class"] in {None, "query", "bulk"},
            f"qualification transition {kind} has an invalid active request class",
        )
        return values


def _client_death_assertions(
    evidence: ScenarioAssertionEvidence,
) -> dict[str, bool]:
    active = evidence.scheduler("dead_client_work_observed")
    continued = evidence.transition(
        "other_client_continued",
        {"project_identity_sha256"},
    )
    terminated = evidence.transition("client_terminated", {"termination"})
    reclaimed = evidence.scheduler("dead_client_work_reclaimed")
    post = evidence.transition(
        "post_reclaim_other_client_query",
        {"server_instance_id"},
    )
    return {
        "dead_client_queue_and_leases_reclaimed": (
            active["query_depth"] > 0
            and active["bulk_depth"] > 0
            and active["active_request_count"] > 0
            and active["lease_count"] > 0
            and reclaimed["query_depth"] == 0
            and reclaimed["bulk_depth"] == 0
            and reclaimed["active_request_count"] == 0
            and reclaimed["lease_count"] == 0
            and terminated["termination"] == "terminated"
        ),
        "other_client_continues": (
            HEX_SHA256.fullmatch(str(continued["project_identity_sha256"]))
            is not None
            and post["server_instance_id"] in evidence.snapshot_instances
        ),
        "no_server_replacement": len(evidence.snapshot_instances) == 1,
    }


def _frozen_owner_assertions(
    evidence: ScenarioAssertionEvidence,
) -> dict[str, bool]:
    bounded = evidence.transition(
        "bounded_owner_unresponsive",
        {
            "started_ns",
            "finished_ns",
            "error_code",
            "timeout_ms",
            "clock_domain",
            "clock_boot_id",
            "retry",
        },
    )
    stable = evidence.transition(
        "owner_identity_stable",
        {
            "server_instance_id",
            "lifetime_authority_id",
            "listener_id",
            "pid",
            "process_start_id",
            "post_release_query_succeeded",
        },
    )
    started = require_nonnegative_int(
        bounded["started_ns"],
        "frozen owner started_ns",
    )
    finished = require_nonnegative_int(
        bounded["finished_ns"],
        "frozen owner finished_ns",
    )
    timeout_ms = require_positive_int(
        bounded["timeout_ms"],
        "frozen owner timeout_ms",
    )
    stable_pid = require_positive_int(stable["pid"], "frozen owner stable pid")
    retry = validate_retry_state(bounded["retry"], "frozen owner retry")
    stable_identity = (
        len(evidence.snapshot_instances) == 1
        and stable["server_instance_id"] == next(iter(evidence.snapshot_instances))
        and len(evidence.snapshot_authorities) == 1
        and (
            stable["lifetime_authority_id"],
            stable["listener_id"],
        )
        == next(iter(evidence.snapshot_authorities))
        and all(
            snapshot["process"]["pid"] == stable_pid
            and snapshot["process"]["process_start_id"]
            == stable["process_start_id"]
            for snapshot in evidence.snapshots
        )
        and stable["post_release_query_succeeded"] is True
    )
    return {
        "owner_unresponsive_is_bounded": (
            finished >= started
            and finished - started <= timeout_ms * 1_000_000
            and bounded["clock_domain"] == "awake_monotonic"
            and bool(bounded["clock_boot_id"])
            and bounded["error_code"] == "embedding_server_owner_unresponsive"
            and retry.code == bounded["error_code"]
            and retry.retry_class == "after_server_change"
            and bool(retry.retry_condition)
        ),
        "authority_retained": stable_identity,
        "no_unlink": stable_identity,
        "no_pid_kill": stable_identity,
        "no_takeover": stable_identity,
        "no_second_engine": len(evidence.snapshot_engines) == 1,
    }


def _incompatible_owner_assertions(
    evidence: ScenarioAssertionEvidence,
) -> dict[str, bool]:
    active = evidence.transition(
        "active_owner_rejected",
        {"compatibility_evidence", "error_code", "retry"},
    )
    idle = evidence.transition(
        "idle_owner_draining",
        {"compatibility_evidence", "error_code", "retry"},
    )
    replacement = evidence.transition(
        "compatible_replacement",
        {"old_server_instance_id", "new_server_instance_id"},
    )
    replaced = (
        replacement["old_server_instance_id"]
        != replacement["new_server_instance_id"]
        and {
            replacement["old_server_instance_id"],
            replacement["new_server_instance_id"],
        }
        <= evidence.snapshot_instances
    )
    active_retry = validate_retry_state(
        active["retry"],
        "incompatible active retry",
    )
    idle_retry = validate_retry_state(
        idle["retry"],
        "incompatible idle retry",
    )
    expected_condition = "the incompatible server exits while fully idle"
    return {
        "idle_owner_drains": (
            idle["compatibility_evidence"] == "injected_contract_mismatch"
            and idle["error_code"] == "embedding_server_draining"
            and idle_retry.code == idle["error_code"]
            and idle_retry.retry_class == "after_owner_idle"
            and idle_retry.retry_after_ms == 0
            and idle_retry.retry_condition == expected_condition
            and replaced
        ),
        "active_owner_returns_typed_retry": (
            active["compatibility_evidence"] == "injected_contract_mismatch"
            and active["error_code"]
            == "embedding_server_incompatible_active_owner"
            and active_retry.code == active["error_code"]
            and active_retry.retry_class == "after_owner_idle"
            and active_retry.retry_after_ms == 0
            and active_retry.retry_condition == expected_condition
        ),
        "one_authority": len(evidence.snapshot_authorities) <= 2 and replaced,
        "one_engine_maximum": len(evidence.snapshot_instances) == 2 and replaced,
    }


def _server_crash_assertions(
    evidence: ScenarioAssertionEvidence,
) -> dict[str, bool]:
    active = evidence.scheduler("inflight_request_observed")
    replacement = evidence.transition(
        "server_replaced",
        {"old_server_instance_id", "new_server_instance_id"},
    )
    replay = evidence.transition(
        "query_replayed",
        {
            "logical_operation_count",
            "wire_attempt_count",
            "wire_attempts",
        },
    )
    attempts = validate_replay_attempts(
        replay,
        old_server_instance_id=replacement["old_server_instance_id"],
        new_server_instance_id=replacement["new_server_instance_id"],
    )
    return {
        "one_replacement_server": (
            active["active_request_class"] == "query"
            and replacement["old_server_instance_id"]
            != replacement["new_server_instance_id"]
            and [attempt.server_instance_id for attempt in attempts]
            == [
                replacement["old_server_instance_id"],
                replacement["new_server_instance_id"],
            ]
        ),
        "pure_embedding_rpc_replayed_at_most_once": (
            replay["logical_operation_count"] == 1
            and replay["wire_attempt_count"] <= 2
            and sum(attempt.outcome == "completed" for attempt in attempts) == 1
        ),
    }


@dataclass(frozen=True)
class ColdRaceElection:
    instances: frozenset[str]
    authorities: frozenset[tuple[str, str]]
    engines: frozenset[tuple[str, str, int, int]]


def _cold_race_election(
    evidence: ScenarioAssertionEvidence,
) -> ColdRaceElection:
    witnesses = {
        phase: [
            observation
            for observation in evidence.process_observations
            if observation.get("phase") == phase
        ]
        for phase in ("cold_race_first", "cold_race_second")
    }
    require(
        all(len(phase_witnesses) == 1 for phase_witnesses in witnesses.values()),
        "cold race must retain exactly one post-reset snapshot from each process",
    )
    snapshots = tuple(
        witnesses[phase][0]["snapshot"]
        for phase in ("cold_race_first", "cold_race_second")
    )
    require(
        all(
            isinstance(snapshot, dict) and snapshot.get("engine") is not None
            for snapshot in snapshots
        ),
        "cold race post-reset snapshots must retain engine identity",
    )
    return ColdRaceElection(
        instances=frozenset(
            snapshot["process"]["server_instance_id"] for snapshot in snapshots
        ),
        authorities=frozenset(
            (
                snapshot["authority"]["lifetime_authority_id"],
                snapshot["authority"]["listener_id"],
            )
            for snapshot in snapshots
        ),
        engines=frozenset(
            (
                snapshot["engine"]["engine_owner_id"],
                snapshot["engine"]["native_worker_id"],
                snapshot["engine"]["load_generation"],
                snapshot["engine"]["model_load_count"],
            )
            for snapshot in snapshots
        ),
    )


def _cold_race_assertions(
    evidence: ScenarioAssertionEvidence,
) -> dict[str, bool]:
    election = _cold_race_election(evidence)
    independent = evidence.transition(
        "two_independent_processes",
        {
            "first_pid",
            "second_pid",
            "first_project_identity_sha256",
            "second_project_identity_sha256",
            "first_transport_peer_verified",
            "second_transport_peer_verified",
        },
    )
    converged = evidence.transition(
        "single_server_convergence",
        {"server_instance_id", "lifetime_authority_id"},
    )
    hosts = (
        evidence.same_account.get("plugin_hosts")
        if isinstance(evidence.same_account, dict)
        else None
    )
    return {
        "two_independent_plugin_hosts": (
            require_positive_int(independent["first_pid"], "cold race first pid")
            != require_positive_int(independent["second_pid"], "cold race second pid")
            and independent["first_transport_peer_verified"] is True
            and independent["second_transport_peer_verified"] is True
        ),
        "same_os_account": (
            evidence.same_account.get("relation") == "same_os_account"
            and isinstance(hosts, list)
            and len(hosts) == 2
        ),
        "different_repositories": (
            independent["first_project_identity_sha256"]
            != independent["second_project_identity_sha256"]
            and all(
                HEX_SHA256.fullmatch(str(independent[field])) is not None
                for field in (
                    "first_project_identity_sha256",
                    "second_project_identity_sha256",
                )
            )
        ),
        "one_lifetime_authority": (
            len(election.authorities) == 1
            and converged["lifetime_authority_id"]
            == next(iter(election.authorities))[0]
        ),
        "one_listener": (
            len({identity[1] for identity in election.authorities}) == 1
        ),
        "one_server": (
            len(election.instances) == 1
            and converged["server_instance_id"] == next(iter(election.instances))
        ),
        "one_engine_owner": (
            len({identity[0] for identity in election.engines}) == 1
        ),
        "one_native_worker": (
            len({identity[1] for identity in election.engines}) == 1
        ),
        "one_load_generation": (
            len({identity[2] for identity in election.engines}) == 1
        ),
        "one_model_load": (
            len(election.engines) == 1 and next(iter(election.engines))[3] == 1
        ),
    }


def _mixed_queue_capacity_is_typed(capacity: dict) -> bool:
    for queue_class in ("query", "bulk"):
        record = capacity[f"{queue_class}_65th"]
        pressure = (
            record.get("error", {}).get("capacity")
            if isinstance(record, dict)
            else None
        )
        if not (
            isinstance(pressure, dict)
            and pressure.get("queue_class") == queue_class
            and pressure.get("capacity") == 64
            and pressure.get("depth") == 64
            and bool(pressure.get("retry_condition"))
        ):
            return False
    return True


def _mixed_queue_is_fifo(class_orders: dict) -> bool:
    return all(
        class_orders[f"{queue_class}_expected_queue_insertion_request_ids"]
        == class_orders[f"{queue_class}_native_completed_request_ids"]
        and isinstance(
            class_orders[
                f"{queue_class}_expected_queue_insertion_request_ids"
            ],
            list,
        )
        and bool(
            class_orders[
                f"{queue_class}_expected_queue_insertion_request_ids"
            ]
        )
        and isinstance(
            class_orders[f"{queue_class}_native_completion_sequences"],
            list,
        )
        and bool(class_orders[f"{queue_class}_native_completion_sequences"])
        and all(
            isinstance(sequence, int)
            and not isinstance(sequence, bool)
            and sequence > 0
            for sequence in class_orders[
                f"{queue_class}_native_completion_sequences"
            ]
        )
        and class_orders[f"{queue_class}_native_completion_sequences"]
        == sorted(class_orders[f"{queue_class}_native_completion_sequences"])
        and len(
            set(class_orders[f"{queue_class}_native_completion_sequences"])
        )
        == len(class_orders[f"{queue_class}_native_completion_sequences"])
        for queue_class in ("query", "bulk")
    )


def _mixed_queue_preserves_project_order(project_orders: dict) -> bool:
    return all(
        project_orders[
            f"{queue_class}_expected_queue_insertion_project_identities"
        ]
        == project_orders[f"{queue_class}_native_completed_project_identities"]
        and len(
            set(
                project_orders[
                    f"{queue_class}_expected_queue_insertion_project_identities"
                ]
            )
        )
        == 2
        for queue_class in ("query", "bulk")
    )


def _mixed_queue_assertions(
    evidence: ScenarioAssertionEvidence,
) -> dict[str, bool]:
    saturated = evidence.scheduler("queues_saturated")
    selected = evidence.scheduler("query_selected_before_bulk_backlog")
    capacity = evidence.transition(
        "typed_capacity_retry_observed",
        {"query_65th", "bulk_65th"},
    )
    class_orders = evidence.transition(
        "per_class_fifo_observed",
        {
            "query_expected_queue_insertion_request_ids",
            "query_native_completed_request_ids",
            "query_native_completion_sequences",
            "bulk_expected_queue_insertion_request_ids",
            "bulk_native_completed_request_ids",
            "bulk_native_completion_sequences",
        },
    )
    project_orders = evidence.transition(
        "global_fifo_across_projects",
        {
            "query_expected_queue_insertion_project_identities",
            "query_native_completed_project_identities",
            "bulk_expected_queue_insertion_project_identities",
            "bulk_native_completed_project_identities",
        },
    )
    preference = evidence.transition(
        "query_preference_observed",
        {
            "first_query_request_id",
            "first_query_native_completion_sequence",
            "first_bulk_request_id",
            "first_bulk_native_completion_sequence",
        },
    )
    resumed = evidence.transition(
        "bulk_resumed",
        {
            "last_query_request_id",
            "last_query_native_completion_sequence",
            "last_bulk_request_id",
            "last_bulk_native_completion_sequence",
        },
    )
    return {
        "query_and_bulk_capacities_are_64": (
            saturated["query_capacity"] == saturated["query_depth"] == 64
            and saturated["bulk_capacity"] == saturated["bulk_depth"] == 64
        ),
        "fifo_within_each_class": _mixed_queue_is_fifo(class_orders),
        "query_preferred_between_bulk_batches": (
            selected["active_request_class"] == "query"
            and selected["bulk_depth"] > 0
            and preference["first_query_native_completion_sequence"]
            < preference["first_bulk_native_completion_sequence"]
        ),
        "bulk_resumes_when_query_queue_permits": (
            resumed["last_bulk_native_completion_sequence"]
            > resumed["last_query_native_completion_sequence"]
        ),
        "no_project_or_scope_round_robin": (
            _mixed_queue_preserves_project_order(project_orders)
        ),
        "typed_retry_names_useful_condition": (
            _mixed_queue_capacity_is_typed(capacity)
        ),
        "no_project_or_request_text_leakage": all(
            all(HEX_SHA256.fullmatch(str(value)) is not None for value in values)
            for key, values in project_orders.items()
            if key.endswith("_project_identities")
        ),
    }


@dataclass(frozen=True)
class WorkerStallMeasurements:
    old_pid: int
    marker_sha256: str
    watchdog_observed_ns: int
    watchdog_last_progress_ns: int
    hard_no_progress_ms: int
    watchdog_cadence_ms: int


def _worker_stall_measurements(fail_stop: dict) -> WorkerStallMeasurements:
    require_nonnegative_int(
        fail_stop["watchdog_progress_sequence"],
        "worker stall watchdog progress sequence",
    )
    return WorkerStallMeasurements(
        old_pid=require_positive_int(
            fail_stop["old_pid"],
            "worker stall old pid",
        ),
        marker_sha256=require_sha256(
            fail_stop["watchdog_marker_sha256"],
            "worker stall watchdog marker",
        ),
        watchdog_observed_ns=require_nonnegative_int(
            fail_stop["watchdog_observed_ns"],
            "worker stall watchdog observed_ns",
        ),
        watchdog_last_progress_ns=require_nonnegative_int(
            fail_stop["watchdog_last_progress_ns"],
            "worker stall watchdog last progress",
        ),
        hard_no_progress_ms=require_positive_int(
            fail_stop["hard_native_no_progress_ms"],
            "worker stall hard no-progress bound",
        ),
        watchdog_cadence_ms=require_positive_int(
            fail_stop["watchdog_cadence_ms"],
            "worker stall watchdog cadence",
        ),
    )


def _worker_stall_assertions(
    evidence: ScenarioAssertionEvidence,
) -> dict[str, bool]:
    active = evidence.scheduler("stalled_request_observed")
    fail_stop = evidence.transition(
        "watchdog_fail_stop_observed",
        {
            "old_pid",
            "old_server_instance_id",
            "wire_attempt_count",
            "wire_attempts",
            "watchdog_marker_sha256",
            "watchdog_reason",
            "watchdog_observed_ns",
            "watchdog_last_progress_ns",
            "watchdog_progress_sequence",
            "hard_native_no_progress_ms",
            "watchdog_cadence_ms",
        },
    )
    replacement = evidence.transition(
        "post_stall_replacement",
        {"new_server_instance_id"},
    )
    survivor = evidence.transition(
        "unrelated_process_survived",
        {"pid", "process_start_id", "new_server_instance_id"},
    )
    measurements = _worker_stall_measurements(fail_stop)
    attempts = validate_replay_attempts(
        fail_stop,
        old_server_instance_id=fail_stop["old_server_instance_id"],
        new_server_instance_id=replacement["new_server_instance_id"],
    )
    replacement_observed = (
        replacement["new_server_instance_id"] in evidence.snapshot_instances
        and all(
            snapshot["process"]["pid"] != measurements.old_pid
            for snapshot in evidence.snapshots[-1:]
        )
    )
    return {
        "independent_watchdog_fail_stops_server": (
            active["active_request_class"] == "bulk"
            and bool(measurements.marker_sha256)
            and fail_stop["watchdog_reason"] == "embedding_engine_stalled"
            and measurements.watchdog_observed_ns
            >= measurements.watchdog_last_progress_ns
            and measurements.watchdog_observed_ns
            - measurements.watchdog_last_progress_ns
            >= measurements.hard_no_progress_ms * 1_000_000
            and measurements.watchdog_cadence_ms
            < measurements.hard_no_progress_ms
            and attempts[0].outcome == "server_loss"
            and replacement_observed
        ),
        "unrelated_process_survives": (
            replacement_observed
            and require_positive_int(
                survivor["pid"],
                "worker stall survivor pid",
            )
            != measurements.old_pid
            and bool(survivor["process_start_id"])
            and survivor["new_server_instance_id"]
            == replacement["new_server_instance_id"]
            and any(
                invocation.get("pid") == survivor["pid"]
                and invocation.get("process_start_id")
                == survivor["process_start_id"]
                and invocation.get("operation") == "query"
                and invocation.get("termination") == "exited"
                for invocation in evidence.invocations
            )
        ),
        "pure_embedding_rpc_replayed_at_most_once": (
            fail_stop["wire_attempt_count"] <= 2
            and sum(attempt.outcome == "completed" for attempt in attempts) == 1
        ),
    }


@dataclass(frozen=True)
class TrueIdleTransitions:
    active: dict
    preserved: dict
    reclaimed: dict
    waited: dict
    idle_surfaces: dict
    absent: dict
    respawned: dict

    @classmethod
    def from_evidence(
        cls,
        evidence: ScenarioAssertionEvidence,
    ) -> TrueIdleTransitions:
        return cls(
            active=evidence.scheduler("anti_idle_work_observed"),
            preserved=evidence.transition(
                "owner_preserved_across_idle_boundary",
                {
                    "held_started_ns",
                    "held_observed_ns",
                    "contract_idle_timeout_ms",
                    "server_instance_id",
                },
            ),
            reclaimed=evidence.scheduler("anti_idle_work_reclaimed"),
            waited=evidence.transition(
                "true_idle_wait",
                {
                    "server_idle_epoch_ns",
                    "server_idle_elapsed_before_client_wait_ns",
                    "client_wait_required_ns",
                    "client_wait_elapsed_ns",
                    "contract_idle_timeout_ms",
                    "clock_boot_id",
                },
            ),
            idle_surfaces=evidence.transition(
                "idle_surfaces_exercised",
                {
                    "diagnostic_count",
                    "idle_connection_close_count",
                    "last_diagnostic_client_elapsed_ns",
                    "last_idle_connection_close_client_elapsed_ns",
                },
            ),
            absent=evidence.transition(
                "owner_absent_after_true_idle",
                {"old_server_instance_id"},
            ),
            respawned=evidence.transition(
                "server_respawned",
                {
                    "new_server_instance_id",
                    "load_generation",
                    "model_load_count",
                    "materialized_model_sha256",
                    "materialized_reused",
                },
            ),
        )


@dataclass(frozen=True)
class TrueIdleWitnesses:
    absent_observed_ns: int
    absent_transition_ns: int
    respawn_observed_ns: int
    respawn_transition_ns: int
    respawn_snapshot: dict
    post_absence_invocations: tuple[dict, ...]


def _true_idle_witnesses(
    evidence: ScenarioAssertionEvidence,
) -> TrueIdleWitnesses:
    absent_observations = [
        observation
        for observation in evidence.process_observations
        if observation.get("phase") == "true_idle_after_wait"
    ]
    require(
        len(absent_observations) == 1
        and absent_observations[0].get("snapshot") is None,
        "true idle must retain exactly one absent-owner witness",
    )
    respawn_observations = [
        observation
        for observation in evidence.process_observations
        if observation.get("phase") == "true_idle_respawned"
    ]
    require(
        len(respawn_observations) == 1
        and isinstance(respawn_observations[0].get("snapshot"), dict)
        and respawn_observations[0]["snapshot"].get("engine") is not None,
        "true idle must retain exactly one replacement-engine witness",
    )
    absent_transition = evidence.observations_by_kind[
        "owner_absent_after_true_idle"
    ][0]
    respawn_transition = evidence.observations_by_kind["server_respawned"][0]
    absent_transition_ns = require_nonnegative_int(
        absent_transition.get("observed_ns"),
        "true idle absence transition time",
    )
    respawn_transition_ns = require_nonnegative_int(
        respawn_transition.get("observed_ns"),
        "true idle respawn transition time",
    )
    return TrueIdleWitnesses(
        absent_observed_ns=require_nonnegative_int(
            absent_observations[0].get("observed_ns"),
            "true idle absent-owner witness time",
        ),
        absent_transition_ns=absent_transition_ns,
        respawn_observed_ns=require_nonnegative_int(
            respawn_observations[0].get("observed_ns"),
            "true idle replacement witness time",
        ),
        respawn_transition_ns=respawn_transition_ns,
        respawn_snapshot=respawn_observations[0]["snapshot"],
        post_absence_invocations=tuple(
            invocation
            for invocation in evidence.invocations
            if isinstance(invocation.get("started_ns"), int)
            and not isinstance(invocation.get("started_ns"), bool)
            and absent_transition_ns
            <= invocation["started_ns"]
            <= respawn_transition_ns
        ),
    )


@dataclass(frozen=True)
class TrueIdleMeasurements:
    timeout_ms: int
    diagnostic_count: int
    idle_connection_close_count: int
    last_diagnostic_client_elapsed_ns: int
    last_idle_connection_close_client_elapsed_ns: int
    server_idle_elapsed_before_client_wait_ns: int
    client_wait_required_ns: int
    client_wait_elapsed_ns: int
    respawn_load_generation: int
    respawn_model_load_count: int
    respawn_materialized_sha256: str


def _true_idle_measurements(
    transitions: TrueIdleTransitions,
) -> TrueIdleMeasurements:
    require_nonnegative_int(
        transitions.waited["server_idle_epoch_ns"],
        "true idle server epoch",
    )
    return TrueIdleMeasurements(
        timeout_ms=require_positive_int(
            transitions.waited["contract_idle_timeout_ms"],
            "true idle contract timeout",
        ),
        diagnostic_count=require_positive_int(
            transitions.idle_surfaces["diagnostic_count"],
            "true idle diagnostic count",
        ),
        idle_connection_close_count=require_positive_int(
            transitions.idle_surfaces["idle_connection_close_count"],
            "true idle connection close count",
        ),
        last_diagnostic_client_elapsed_ns=require_nonnegative_int(
            transitions.idle_surfaces["last_diagnostic_client_elapsed_ns"],
            "true idle last diagnostic ns",
        ),
        last_idle_connection_close_client_elapsed_ns=require_nonnegative_int(
            transitions.idle_surfaces[
                "last_idle_connection_close_client_elapsed_ns"
            ],
            "true idle last connection close ns",
        ),
        server_idle_elapsed_before_client_wait_ns=require_nonnegative_int(
            transitions.waited["server_idle_elapsed_before_client_wait_ns"],
            "true idle server elapsed before local wait",
        ),
        client_wait_required_ns=require_nonnegative_int(
            transitions.waited["client_wait_required_ns"],
            "true idle client wait required",
        ),
        client_wait_elapsed_ns=require_nonnegative_int(
            transitions.waited["client_wait_elapsed_ns"],
            "true idle client wait elapsed",
        ),
        respawn_load_generation=require_positive_int(
            transitions.respawned["load_generation"],
            "true idle respawn load generation",
        ),
        respawn_model_load_count=require_positive_int(
            transitions.respawned["model_load_count"],
            "true idle respawn model load count",
        ),
        respawn_materialized_sha256=require_sha256(
            transitions.respawned["materialized_model_sha256"],
            "true idle respawn materialized model",
        ),
    )


def _true_idle_assertions(
    evidence: ScenarioAssertionEvidence,
) -> dict[str, bool]:
    transitions = TrueIdleTransitions.from_evidence(evidence)
    witnesses = _true_idle_witnesses(evidence)
    measurements = _true_idle_measurements(transitions)
    invocation = (
        witnesses.post_absence_invocations[0]
        if len(witnesses.post_absence_invocations) == 1
        else None
    )
    respawn_engine = witnesses.respawn_snapshot["engine"]
    return {
        "queued_active_and_leased_work_prevent_exit": (
            transitions.active["query_depth"] > 0
            and transitions.active["bulk_depth"] > 0
            and transitions.active["active_request_count"] > 0
            and transitions.active["lease_count"] > 0
            and transitions.preserved["server_instance_id"]
            == transitions.absent["old_server_instance_id"]
            and transitions.preserved["held_observed_ns"]
            - transitions.preserved["held_started_ns"]
            >= transitions.preserved["contract_idle_timeout_ms"] * 1_000_000
        ),
        "idle_connections_and_diagnostics_do_not_extend_idle": (
            measurements.diagnostic_count >= 2
            and measurements.idle_connection_close_count >= 2
            and measurements.last_diagnostic_client_elapsed_ns
            >= measurements.timeout_ms * 500_000
            and measurements.last_idle_connection_close_client_elapsed_ns
            >= measurements.timeout_ms * 500_000
            and bool(transitions.waited["clock_boot_id"])
        ),
        "exit_after_60000_awake_ms": (
            measurements.timeout_ms == 60_000
            and measurements.server_idle_elapsed_before_client_wait_ns
            + measurements.client_wait_required_ns
            >= measurements.timeout_ms * 1_000_000
            and measurements.client_wait_elapsed_ns
            >= measurements.client_wait_required_ns
            and transitions.reclaimed["query_depth"] == 0
            and transitions.reclaimed["bulk_depth"] == 0
            and transitions.reclaimed["active_request_count"] == 0
            and transitions.reclaimed["lease_count"] == 0
        ),
        "next_product_operation_respawns_without_consent": (
            transitions.absent["old_server_instance_id"]
            != transitions.respawned["new_server_instance_id"]
            and witnesses.absent_observed_ns <= witnesses.absent_transition_ns
            and invocation is not None
            and invocation.get("operation") == "query"
            and invocation.get("exit_code") == 0
            and invocation.get("termination") == "exited"
            and isinstance(invocation.get("finished_ns"), int)
            and not isinstance(invocation.get("finished_ns"), bool)
            and invocation["started_ns"]
            <= invocation["finished_ns"]
            <= witnesses.respawn_observed_ns
            <= witnesses.respawn_transition_ns
            and witnesses.respawn_snapshot["process"]["server_instance_id"]
            == transitions.respawned["new_server_instance_id"]
        ),
        "verified_materialization_reused": (
            isinstance(evidence.materialization, dict)
            and measurements.respawn_materialized_sha256
            == require_sha256(
                evidence.materialization.get("sha256"),
                "retained materialized model",
            )
            and transitions.respawned["materialized_reused"] is True
            and measurements.respawn_load_generation
            == respawn_engine["load_generation"]
            and measurements.respawn_model_load_count
            == respawn_engine["model_load_count"]
            == 1
        ),
    }


def derive_scenario_assertions(
    scenario_id: str,
    *,
    observations_by_kind: dict[str, list[dict]],
    process_observations: list[dict],
    invocations: list[dict],
    control_actions: list[str],
    same_account: dict,
    materialization: dict,
) -> dict[str, bool]:
    evidence = ScenarioAssertionEvidence.from_raw(
        scenario_id,
        observations_by_kind=observations_by_kind,
        process_observations=process_observations,
        invocations=invocations,
        same_account=same_account,
        materialization=materialization,
    )
    handler = {
        "client_death": _client_death_assertions,
        "cold_race": _cold_race_assertions,
        "frozen_owner": _frozen_owner_assertions,
        "incompatible_owner": _incompatible_owner_assertions,
        "mixed_queue": _mixed_queue_assertions,
        "server_crash": _server_crash_assertions,
        "true_idle_respawn": _true_idle_assertions,
        "worker_stall": _worker_stall_assertions,
    }.get(scenario_id)
    require(handler is not None, f"unknown qualification scenario {scenario_id}")
    assertions = handler(evidence)
    failed = sorted(name for name, value in assertions.items() if value is not True)
    require(
        not failed,
        f"qualification scenario {scenario_id} raw evidence failed assertions: {', '.join(failed)}",
    )
    return assertions


@dataclass(frozen=True)
class QualificationArtifactDocument:
    name: str
    payload_bytes: bytes
    payload: dict


@dataclass(frozen=True)
class QualificationArtifactSummary:
    name: str
    process_count: int
    control_event_count: int
    process_observation_count: int
    observation_count: int
    event_count: int


@dataclass(frozen=True)
class QualificationOrchestration:
    started_ns: int
    finished_ns: int
    invocations: tuple[dict, ...]


@dataclass(frozen=True)
class QualificationControlEvidence:
    events: tuple[dict, ...]
    actions: tuple[str, ...]


@dataclass(frozen=True)
class QualificationTransitionEvidence:
    observations: tuple[dict, ...]
    by_kind: dict[str, list[dict]]


@dataclass(frozen=True)
class QualificationArtifactEvidence:
    summary: QualificationArtifactSummary
    document: QualificationArtifactDocument
    orchestration: QualificationOrchestration
    controls: QualificationControlEvidence
    process_observations: tuple[dict, ...]
    transitions: QualificationTransitionEvidence
    events: tuple[dict, ...]


def _normalized_qualification_summary(
    summary: object,
    *,
    scenario_id: str,
) -> QualificationArtifactSummary:
    require(
        isinstance(summary, dict),
        f"qualification scenario {scenario_id} summary is malformed",
    )
    require_exact_keys(
        summary,
        {
            "artifact",
            "process_count",
            "control_event_count",
            "process_observation_count",
            "observation_count",
            "event_count",
        },
        f"qualification scenario {scenario_id} summary",
    )
    return QualificationArtifactSummary(
        require_nonempty_string(
            summary["artifact"],
            f"qualification scenario {scenario_id} artifact",
        ),
        require_nonnegative_int(
            summary["process_count"],
            f"qualification scenario {scenario_id} summary process_count",
        ),
        require_nonnegative_int(
            summary["control_event_count"],
            f"qualification scenario {scenario_id} summary control_event_count",
        ),
        require_nonnegative_int(
            summary["process_observation_count"],
            f"qualification scenario {scenario_id} summary process_observation_count",
        ),
        require_nonnegative_int(
            summary["observation_count"],
            f"qualification scenario {scenario_id} summary observation_count",
        ),
        require_nonnegative_int(
            summary["event_count"],
            f"qualification scenario {scenario_id} summary event_count",
        ),
    )


def _qualification_artifact_document(
    artifact_root: Path,
    summary: object,
    *,
    scenario_id: str,
    contracts: dict,
    forbidden_values: list[str],
) -> tuple[QualificationArtifactSummary, QualificationArtifactDocument]:
    normalized_summary = _normalized_qualification_summary(
        summary,
        scenario_id=scenario_id,
    )
    name = normalized_summary.name
    relative = Path(name)
    require(
        not relative.is_absolute()
        and len(relative.parts) == 1
        and relative.name == name
        and relative.suffix == ".json",
        f"qualification scenario {scenario_id} artifact must be a JSON basename",
    )
    path = artifact_root / relative
    require(
        path.is_file() and not path.is_symlink(),
        f"qualification artifact is missing or unsafe: {name}",
    )
    require(
        path.resolve().parent == artifact_root.resolve(),
        f"qualification artifact escaped its private output directory: {name}",
    )
    payload_bytes = path.read_bytes()
    for forbidden in forbidden_values:
        require(
            forbidden.encode("utf-8") not in payload_bytes,
            f"qualification artifact {name} leaked private request material",
        )
    try:
        payload = json.loads(payload_bytes)
    except json.JSONDecodeError as exc:
        raise ProofFailure(
            f"qualification artifact {name} is not valid JSON: {exc}"
        ) from exc
    require(
        isinstance(payload, dict),
        f"qualification artifact {name} must be an object",
    )
    require_exact_keys(
        payload,
        {
            "schema_version",
            "scenario",
            "contracts",
            "orchestration",
            "control_events",
            "process_observations",
            "observations",
            "events",
        },
        f"qualification artifact {name}",
    )
    require(
        payload["schema_version"] == 3,
        f"qualification artifact {name} schema is unsupported",
    )
    require(
        payload["scenario"] == scenario_id,
        f"qualification artifact {name} names the wrong scenario",
    )
    require(
        payload["contracts"] == contracts,
        f"qualification artifact {name} used different contracts",
    )
    return normalized_summary, QualificationArtifactDocument(
        name,
        payload_bytes,
        payload,
    )


def _qualification_invocations(
    value: object,
    *,
    name: str,
    started_ns: int,
    finished_ns: int,
) -> tuple[dict, ...]:
    require(
        isinstance(value, list),
        f"qualification artifact {name} process invocations are malformed",
    )
    invocation_ids: set[str] = set()
    for index, invocation in enumerate(value):
        field = f"qualification artifact {name} process invocation {index}"
        require(isinstance(invocation, dict), f"{field} is malformed")
        require_exact_keys(
            invocation,
            {
                "invocation_id",
                "operation",
                "project_identity_sha256",
                "pid",
                "process_start_id",
                "started_ns",
                "finished_ns",
                "exit_code",
                "termination",
            },
            field,
        )
        invocation_id = require_nonempty_string(
            invocation["invocation_id"],
            f"{field}.invocation_id",
        )
        require(
            invocation_id not in invocation_ids,
            f"qualification artifact {name} duplicated invocation {invocation_id}",
        )
        invocation_ids.add(invocation_id)
        require_nonempty_string(invocation["operation"], f"{field}.operation")
        require_sha256(
            invocation["project_identity_sha256"],
            f"{field}.project_identity_sha256",
        )
        require_positive_int(invocation["pid"], f"{field}.pid")
        require_nonempty_string(
            invocation["process_start_id"],
            f"{field}.process_start_id",
        )
        invocation_started = require_nonnegative_int(
            invocation["started_ns"],
            f"{field}.started_ns",
        )
        invocation_finished = require_nonnegative_int(
            invocation["finished_ns"],
            f"{field}.finished_ns",
        )
        require(
            started_ns <= invocation_started <= invocation_finished <= finished_ns,
            f"{field} escaped its block",
        )
        require(
            invocation["exit_code"] is None
            or (
                isinstance(invocation["exit_code"], int)
                and not isinstance(invocation["exit_code"], bool)
            ),
            f"{field}.exit_code is invalid",
        )
        require(
            invocation["termination"] in {"exited", "terminated"},
            f"{field}.termination is invalid",
        )
    return tuple(value)


def _qualification_orchestration(
    value: object,
    *,
    name: str,
) -> QualificationOrchestration:
    field = f"qualification artifact {name} orchestration"
    require(isinstance(value, dict), f"{field} is malformed")
    require_exact_keys(
        value,
        {"started_ns", "finished_ns", "process_invocations"},
        field,
    )
    started_ns = require_nonnegative_int(value["started_ns"], f"{field}.started_ns")
    finished_ns = require_nonnegative_int(value["finished_ns"], f"{field}.finished_ns")
    require(
        finished_ns >= started_ns,
        f"qualification artifact {name} orchestration moved backwards",
    )
    return QualificationOrchestration(
        started_ns,
        finished_ns,
        _qualification_invocations(
            value["process_invocations"],
            name=name,
            started_ns=started_ns,
            finished_ns=finished_ns,
        ),
    )


def _validate_qualification_clock(
    clock: object,
    field: str,
    *,
    observed: bool,
) -> dict:
    require(isinstance(clock, dict), f"{field} is malformed")
    numeric = "observed_ns" if observed else "resolution_ns"
    require_exact_keys(clock, {"domain", "api", "boot_id", numeric}, field)
    require(
        clock["domain"] == "awake_monotonic",
        f"{field} used the wrong clock domain",
    )
    require_nonempty_string(clock["api"], f"{field}.api")
    require_nonempty_string(clock["boot_id"], f"{field}.boot_id")
    require_nonnegative_int(clock[numeric], f"{field}.{numeric}")
    return clock


def _validate_snapshot_identity(
    snapshot: dict,
    *,
    field: str,
    package: dict,
) -> None:
    authority = snapshot["authority"]
    process = snapshot["process"]
    scheduler = snapshot["scheduler"]
    require(
        isinstance(authority, dict)
        and isinstance(process, dict)
        and isinstance(scheduler, dict),
        f"{field} omitted server identity",
    )
    require_exact_keys(
        process,
        {
            "server_instance_id",
            "pid",
            "process_start_id",
            "executable_sha256",
            "executable_version",
        },
        f"{field}.process",
    )
    for identity_field in (
        "endpoint_namespace_id",
        "lifetime_authority_id",
        "listener_id",
    ):
        require_nonempty_string(
            authority.get(identity_field),
            f"{field}.authority.{identity_field}",
        )
    require_nonempty_string(
        process.get("server_instance_id"),
        f"{field}.process.server_instance_id",
    )
    require_positive_int(process.get("pid"), f"{field}.process.pid")
    require_nonempty_string(
        process.get("process_start_id"),
        f"{field}.process.process_start_id",
    )
    require(
        process.get("executable_sha256") == package["executable_sha256"]
        and process.get("executable_version") == package["release_version"],
        f"{field}.process does not match the exact packaged executable",
    )
    require(
        scheduler.get("query_capacity") == 64
        and scheduler.get("bulk_capacity") == 64,
        f"{field} queue capacities differ from the bound contract",
    )


def _validated_qualification_snapshot(
    snapshot: object,
    *,
    field: str,
    contracts: dict,
    package: dict,
) -> dict:
    require(isinstance(snapshot, dict), f"{field} is malformed")
    required = {
        "schema_version",
        "event_sequence",
        "lifecycle",
        "clock",
        "protocol",
        "authority",
        "process",
        "scheduler",
    }
    require(
        required <= set(snapshot)
        and set(snapshot) <= required | {"engine", "failure"},
        f"{field} fields differ from the raw snapshot contract",
    )
    require(snapshot["schema_version"] == 1, f"{field} schema is unsupported")
    require_nonnegative_int(snapshot["event_sequence"], f"{field}.event_sequence")
    require(
        snapshot["lifecycle"] in SERVER_LIFECYCLES,
        f"{field} lifecycle is invalid",
    )
    _validate_qualification_clock(snapshot["clock"], f"{field}.clock", observed=False)
    protocol = snapshot["protocol"]
    require(isinstance(protocol, dict), f"{field}.protocol is malformed")
    for contract_field, expected in contracts.items():
        require(
            protocol.get(contract_field) == expected,
            f"{field}.protocol.{contract_field} is stale",
        )
    _validate_snapshot_identity(snapshot, field=field, package=package)
    engine = snapshot.get("engine")
    if engine is not None:
        require(isinstance(engine, dict), f"{field}.engine is malformed")
        for identity_field in ("engine_owner_id", "native_worker_id"):
            require_nonempty_string(
                engine.get(identity_field),
                f"{field}.engine.{identity_field}",
            )
        require_positive_int(
            engine.get("load_generation"),
            f"{field}.engine.load_generation",
        )
        require_positive_int(
            engine.get("model_load_count"),
            f"{field}.engine.model_load_count",
        )
    return snapshot


def _qualification_controls(
    value: object,
    *,
    name: str,
    contracts: dict,
    package: dict,
    nonce_sha256: str,
) -> QualificationControlEvidence:
    require(
        isinstance(value, list),
        f"qualification artifact {name} control events are malformed",
    )
    previous_sequence = -1
    actions = []
    allowed_actions = {
        "crash_server",
        "stall_native",
        "release_native",
        "hold_class",
        "release_class",
        "force_incompatible",
        "clear_incompatible",
        "snapshot",
        "freeze_owner",
        "release_owner",
    }
    for index, event in enumerate(value):
        field = f"qualification artifact {name} control event {index}"
        require(isinstance(event, dict), f"{field} is malformed")
        required = {
            "schema_version",
            "sequence",
            "action",
            "status",
            "authenticated_nonce_sha256",
            "server_event_sequence",
            "clock",
        }
        require(
            required <= set(event) and set(event) <= required | {"snapshot", "details"},
            f"{field} fields are invalid",
        )
        require(
            event["schema_version"] == 1,
            f"qualification artifact {name} control event schema is unsupported",
        )
        sequence = require_nonnegative_int(event["sequence"], f"{field}.sequence")
        require(
            sequence > previous_sequence,
            f"qualification artifact {name} control event sequence is not increasing",
        )
        previous_sequence = sequence
        action = require_nonempty_string(event["action"], f"{field}.action")
        require(
            action in allowed_actions,
            f"qualification artifact {name} used unknown control {action}",
        )
        actions.append(action)
        _validate_qualification_control_details(
            event,
            field=field,
            contracts=contracts,
            package=package,
            nonce_sha256=nonce_sha256,
        )
    return QualificationControlEvidence(tuple(value), tuple(actions))


def _validate_qualification_control_details(
    event: dict,
    *,
    field: str,
    contracts: dict,
    package: dict,
    nonce_sha256: str,
) -> None:
    require(
        event["status"] in {"completed", "accepted"},
        f"{field} did not complete",
    )
    require(
        event["authenticated_nonce_sha256"] == nonce_sha256,
        f"{field} was not authenticated",
    )
    require_nonnegative_int(
        event["server_event_sequence"],
        f"{field}.server_event_sequence",
    )
    _validate_qualification_clock(event["clock"], f"{field}.clock", observed=True)
    if "snapshot" in event:
        _validated_qualification_snapshot(
            event["snapshot"],
            field=f"{field}.snapshot",
            contracts=contracts,
            package=package,
        )
    if "details" in event:
        require(
            isinstance(event["details"], dict)
            and all(
                isinstance(key, str) and isinstance(value, str)
                for key, value in event["details"].items()
            ),
            f"{field}.details is malformed",
        )


_PROCESS_OBSERVATION_FIELDS = {
    "phase",
    "observed_ns",
    "server_instance_id",
    "pid",
    "process_start_id",
    "executable_sha256",
    "executable_version",
    "endpoint_namespace_id",
    "lifetime_authority_id",
    "listener_id",
    "protocol_sha256",
    "constant_set_sha256",
    "measurement_protocol_sha256",
    "load_generation",
    "snapshot",
}


def _validate_present_process_observation(
    observation: dict,
    snapshot: dict,
    *,
    field: str,
    contracts: dict,
) -> None:
    for contract_field, expected in contracts.items():
        require(
            observation[contract_field] == expected,
            f"{field}.{contract_field} is stale",
        )
    require(
        observation["server_instance_id"] == snapshot["process"]["server_instance_id"]
        and observation["pid"] == snapshot["process"]["pid"]
        and observation["process_start_id"] == snapshot["process"]["process_start_id"]
        and observation["executable_sha256"]
        == snapshot["process"]["executable_sha256"]
        and observation["executable_version"]
        == snapshot["process"]["executable_version"]
        and observation["endpoint_namespace_id"]
        == snapshot["authority"]["endpoint_namespace_id"]
        and observation["lifetime_authority_id"]
        == snapshot["authority"]["lifetime_authority_id"]
        and observation["listener_id"] == snapshot["authority"]["listener_id"],
        f"{field} identity disagrees with its snapshot",
    )
    engine = snapshot.get("engine")
    require(
        observation["load_generation"]
        == (engine.get("load_generation") if isinstance(engine, dict) else None),
        f"{field} load generation disagrees with its snapshot",
    )


def _qualification_process_observations(
    value: object,
    *,
    name: str,
    orchestration: QualificationOrchestration,
    contracts: dict,
    package: dict,
) -> tuple[dict, ...]:
    require(
        isinstance(value, list),
        f"qualification artifact {name} process observations are malformed",
    )
    for index, observation in enumerate(value):
        field = f"qualification artifact {name} process observation {index}"
        require(isinstance(observation, dict), f"{field} is malformed")
        require_exact_keys(observation, _PROCESS_OBSERVATION_FIELDS, field)
        require_nonempty_string(observation["phase"], f"{field}.phase")
        observed_ns = require_nonnegative_int(
            observation["observed_ns"],
            f"{field}.observed_ns",
        )
        require(
            orchestration.started_ns <= observed_ns <= orchestration.finished_ns,
            f"{field} escaped its block",
        )
        snapshot = observation["snapshot"]
        if snapshot is None:
            require(
                all(
                    observation[item] is None
                    for item in _PROCESS_OBSERVATION_FIELDS
                    - {"phase", "observed_ns", "snapshot"}
                ),
                f"qualification artifact {name} absent observation retained an identity",
            )
            continue
        normalized_snapshot = _validated_qualification_snapshot(
            snapshot,
            field=f"{field}.snapshot",
            contracts=contracts,
            package=package,
        )
        _validate_present_process_observation(
            observation,
            normalized_snapshot,
            field=field,
            contracts=contracts,
        )
    return tuple(value)


def _qualification_transitions(
    value: object,
    *,
    name: str,
    orchestration: QualificationOrchestration,
) -> QualificationTransitionEvidence:
    require(
        isinstance(value, list),
        f"qualification artifact {name} observations are malformed",
    )
    by_kind: dict[str, list[dict]] = {}
    for index, observation in enumerate(value):
        field = f"qualification artifact {name} observation {index}"
        require(isinstance(observation, dict), f"{field} is malformed")
        require_exact_keys(
            observation,
            {"sequence", "kind", "observed_ns", "values"},
            field,
        )
        require(
            observation["sequence"] == index,
            f"qualification artifact {name} observation sequence is not contiguous",
        )
        kind = require_nonempty_string(observation["kind"], f"{field}.kind")
        observed_ns = require_nonnegative_int(
            observation["observed_ns"],
            f"{field}.observed_ns",
        )
        require(
            orchestration.started_ns <= observed_ns <= orchestration.finished_ns,
            f"{field} escaped its block",
        )
        require(
            isinstance(observation["values"], dict),
            f"{field}.values is malformed",
        )
        by_kind.setdefault(kind, []).append(observation)
    return QualificationTransitionEvidence(tuple(value), by_kind)


_REQUIRED_TRANSITIONS = {
    "client_death": {
        "dead_client_work_observed",
        "other_client_continued",
        "client_terminated",
        "dead_client_work_reclaimed",
        "post_reclaim_other_client_query",
    },
    "cold_race": {"two_independent_processes", "single_server_convergence"},
    "frozen_owner": {"bounded_owner_unresponsive", "owner_identity_stable"},
    "incompatible_owner": {
        "active_owner_rejected",
        "idle_owner_draining",
        "compatible_replacement",
    },
    "mixed_queue": {
        "queues_saturated",
        "query_selected_before_bulk_backlog",
        "typed_capacity_retry_observed",
        "per_class_fifo_observed",
        "global_fifo_across_projects",
        "query_preference_observed",
        "bulk_resumed",
    },
    "server_crash": {
        "inflight_request_observed",
        "server_replaced",
        "query_replayed",
    },
    "true_idle_respawn": {
        "anti_idle_work_observed",
        "owner_preserved_across_idle_boundary",
        "anti_idle_work_reclaimed",
        "true_idle_wait",
        "idle_surfaces_exercised",
        "owner_absent_after_true_idle",
        "server_respawned",
    },
    "worker_stall": {
        "stalled_request_observed",
        "watchdog_fail_stop_observed",
        "unrelated_process_survived",
        "post_stall_replacement",
    },
}

_REQUIRED_CONTROLS = {
    "client_death": Counter({"hold_class": 2, "release_class": 2}),
    "cold_race": Counter(),
    "frozen_owner": Counter({"freeze_owner": 1, "release_owner": 1}),
    "incompatible_owner": Counter(
        {"force_incompatible": 1, "clear_incompatible": 1}
    ),
    "mixed_queue": Counter({"hold_class": 2, "release_class": 2}),
    "server_crash": Counter({"hold_class": 1, "crash_server": 1}),
    "true_idle_respawn": Counter({"hold_class": 2, "release_class": 2}),
    "worker_stall": Counter({"stall_native": 1, "release_native": 1}),
}


def _verify_cold_race_evidence(
    *,
    name: str,
    process_observations: tuple[dict, ...],
    transitions: QualificationTransitionEvidence,
) -> None:
    require(
        any(
            observation["phase"] == "cold_race_no_owner"
            and observation["snapshot"] is None
            for observation in process_observations
        ),
        f"qualification artifact {name} did not prove owner absence before the race",
    )
    independent = transitions.by_kind["two_independent_processes"][0]["values"]
    require(
        independent.get("first_pid") != independent.get("second_pid")
        and independent.get("first_project_identity_sha256")
        != independent.get("second_project_identity_sha256")
        and independent.get("first_transport_peer_verified") is True
        and independent.get("second_transport_peer_verified") is True,
        f"qualification artifact {name} cold-race processes were not independent",
    )


def _verify_scenario_artifact_requirements(
    *,
    name: str,
    scenario_id: str,
    controls: QualificationControlEvidence,
    process_observations: tuple[dict, ...],
    transitions: QualificationTransitionEvidence,
) -> None:
    require(
        all(
            len(transitions.by_kind.get(kind, [])) == 1
            for kind in _REQUIRED_TRANSITIONS[scenario_id]
        ),
        f"qualification artifact {name} omitted or duplicated required raw transitions",
    )
    actual_controls = Counter(controls.actions)
    require(
        all(
            actual_controls[action] >= count
            for action, count in _REQUIRED_CONTROLS[scenario_id].items()
        ),
        f"qualification artifact {name} omitted required authenticated controls",
    )
    if scenario_id == "cold_race":
        _verify_cold_race_evidence(
            name=name,
            process_observations=process_observations,
            transitions=transitions,
        )


def _qualification_events(
    value: object,
    *,
    name: str,
    orchestration: QualificationOrchestration,
) -> tuple[dict, ...]:
    require(
        isinstance(value, list) and value,
        f"qualification artifact {name} has no correlated events",
    )
    for index, event in enumerate(value):
        field = f"qualification artifact {name} event {index}"
        require(isinstance(event, dict), f"{field} is malformed")
        require_exact_keys(
            event,
            {
                "sequence",
                "source",
                "action",
                "observed_ns",
                "correlation_id",
                "values",
            },
            field,
        )
        require(
            event["sequence"] == index,
            f"qualification artifact {name} event sequence is not contiguous",
        )
        require_nonempty_string(event["source"], f"{field}.source")
        require_nonempty_string(event["action"], f"{field}.action")
        observed_ns = require_nonnegative_int(event["observed_ns"], f"{field}.observed_ns")
        require(
            orchestration.started_ns <= observed_ns <= orchestration.finished_ns,
            f"{field} escaped its block",
        )
        require(
            event["correlation_id"] is None
            or (
                isinstance(event["correlation_id"], str)
                and bool(event["correlation_id"])
            ),
            f"{field}.correlation_id is malformed",
        )
        require(isinstance(event["values"], dict), f"{field}.values is malformed")
    return tuple(value)


def _verify_qualification_summary(
    *,
    scenario_id: str,
    evidence: QualificationArtifactEvidence,
) -> None:
    expected_counts = {
        "process_count": (
            evidence.summary.process_count,
            len(evidence.orchestration.invocations),
        ),
        "control_event_count": (
            evidence.summary.control_event_count,
            len(evidence.controls.events),
        ),
        "process_observation_count": (
            evidence.summary.process_observation_count,
            len(evidence.process_observations),
        ),
        "observation_count": (
            evidence.summary.observation_count,
            len(evidence.transitions.observations),
        ),
        "event_count": (evidence.summary.event_count, len(evidence.events)),
    }
    for field, (retained, expected) in expected_counts.items():
        require(
            retained == expected,
            f"qualification scenario {scenario_id} summary {field} is stale",
        )


def qualification_artifact(
    artifact_root: Path,
    summary: object,
    *,
    scenario_id: str,
    contracts: dict,
    package: dict,
    same_account: dict,
    materialization: dict,
    nonce_sha256: str,
    forbidden_values: list[str],
) -> tuple[dict, dict]:
    normalized_summary, document = _qualification_artifact_document(
        artifact_root,
        summary,
        scenario_id=scenario_id,
        contracts=contracts,
        forbidden_values=forbidden_values,
    )
    payload = document.payload
    orchestration = _qualification_orchestration(
        payload["orchestration"],
        name=document.name,
    )
    controls = _qualification_controls(
        payload["control_events"],
        name=document.name,
        contracts=contracts,
        package=package,
        nonce_sha256=nonce_sha256,
    )
    process_observations = _qualification_process_observations(
        payload["process_observations"],
        name=document.name,
        orchestration=orchestration,
        contracts=contracts,
        package=package,
    )
    transitions = _qualification_transitions(
        payload["observations"],
        name=document.name,
        orchestration=orchestration,
    )
    _verify_scenario_artifact_requirements(
        name=document.name,
        scenario_id=scenario_id,
        controls=controls,
        process_observations=process_observations,
        transitions=transitions,
    )
    evidence = QualificationArtifactEvidence(
        normalized_summary,
        document,
        orchestration,
        controls,
        process_observations,
        transitions,
        _qualification_events(
            payload["events"],
            name=document.name,
            orchestration=orchestration,
        ),
    )
    _verify_qualification_summary(
        scenario_id=scenario_id,
        evidence=evidence,
    )
    assertions = derive_scenario_assertions(
        scenario_id,
        observations_by_kind=evidence.transitions.by_kind,
        process_observations=list(evidence.process_observations),
        invocations=list(evidence.orchestration.invocations),
        control_actions=list(evidence.controls.actions),
        same_account=same_account,
        materialization=materialization,
    )
    return (
        {
            "name": document.name,
            "sha256": hashlib.sha256(document.payload_bytes).hexdigest(),
        },
        assertions,
    )


def require_candidate_matrix_installation_source(
    cell_id: str | None,
    installation_source: str,
) -> None:
    alias = CANDIDATE_QUALIFICATION_MATRIX_ALIASES.get(cell_id)
    if alias is not None:
        require(
            installation_source == alias["installation_source"],
            "candidate qualification matrix alias requires candidate-installed provenance",
        )


def qualification_measurement_artifact(
    artifact_root: Path,
    summary: object,
    *,
    contracts: dict,
    measurement_contract: dict,
    target: str,
    proof_tier: str,
    matrix_cell_id: str,
    expected_policy: str,
    expected_backend: str,
    forbidden_values: list[str],
) -> dict:
    require(isinstance(summary, dict), "qualification measurement summary is malformed")
    require_exact_keys(
        summary,
        {"artifact", "metric_count", "sample_count"},
        "qualification measurement summary",
    )
    name = require_nonempty_string(summary["artifact"], "qualification measurement artifact")
    relative = Path(name)
    require(
        not relative.is_absolute()
        and len(relative.parts) == 1
        and relative.name == name
        and name == "measurements.raw.json",
        "qualification measurement artifact must be measurements.raw.json",
    )
    path = artifact_root / relative
    require(
        path.is_file() and not path.is_symlink() and path.resolve().parent == artifact_root.resolve(),
        "qualification measurement artifact is missing or unsafe",
    )
    payload_bytes = path.read_bytes()
    for forbidden in forbidden_values:
        require(
            forbidden.encode("utf-8") not in payload_bytes,
            "qualification measurement artifact leaked private request material",
        )
    try:
        payload = json.loads(payload_bytes)
    except json.JSONDecodeError as exc:
        raise ProofFailure(
            f"qualification measurement artifact is not valid JSON: {exc}"
        ) from exc
    require(isinstance(payload, dict), "qualification measurement artifact must be an object")
    require_exact_keys(
        payload,
        {"schema_version", "contracts", "external_metrics", "metrics"},
        "qualification measurement artifact",
    )
    require(payload["schema_version"] == 2, "qualification measurement schema is unsupported")
    require(payload["contracts"] == contracts, "qualification measurements used stale contracts")
    require(
        payload["external_metrics"] == sorted(EXTERNAL_QUALIFICATION_METRICS),
        "qualification measurements changed the externally owned metric set",
    )

    protocol = measurement_contract["measurement_protocol"]
    metric_contracts = protocol["metric_contracts"]
    phase_boundaries = protocol["phase_boundaries"]
    raw_metric_names = set(protocol["required_metrics"]) - EXTERNAL_QUALIFICATION_METRICS
    matrix_cell = selected_qualification_matrix_cell(
        protocol,
        cell_id=matrix_cell_id,
        target=target,
        proof_tier=proof_tier,
        expected_policy=expected_policy,
        expected_backend=expected_backend,
    )
    metrics = payload["metrics"]
    require(
        isinstance(metrics, dict) and set(metrics) == raw_metric_names,
        "qualification measurements did not contain exactly the 12 product-path metrics",
    )
    require(
        summary["metric_count"] == len(raw_metric_names),
        "qualification measurement metric count is stale",
    )
    target_os = TARGET_CONTRACTS[target]["target_os"]
    clock_policy = protocol["clock_policy"]
    allowed_awake_apis = set(clock_policy["platform_apis"][target_os])
    suspend_contract = clock_policy["suspend_detection"]
    inclusive_api = suspend_contract["platform_apis"][target_os]
    maximum_suspend_ns = require_nonnegative_int(
        suspend_contract["maximum_inclusive_minus_awake_ns"],
        "measurement suspend-detection tolerance",
    )
    duration_metrics = raw_metric_names - {
        "bulk_documents_per_second",
        "bulk_tokens_per_second",
        "total_codestory_process_memory",
        "backend_observed_accelerator_residency",
    }
    values: dict[str, float | int] = {}
    sample_count = 0
    for metric in sorted(raw_metric_names):
        record = metrics[metric]
        require(isinstance(record, dict), f"qualification measurement {metric} is malformed")
        require_exact_keys(record, {"unit", "samples"}, f"qualification measurement {metric}")
        require(
            record["unit"] == metric_contracts[metric]["unit"],
            f"qualification measurement {metric} used the wrong unit",
        )
        samples = record["samples"]
        sample_policy = protocol["metric_sampling"][metric]
        require(
            isinstance(samples, list)
            and len(samples) == sample_policy["sample_count"],
            f"qualification measurement {metric} sample count changed",
        )
        sample_count += len(samples)
        sample_values: list[float | int] = []
        sample_ids: set[str] = set()
        server_identities: list[tuple[str, str, int]] = []
        for sample_index, sample in enumerate(samples):
            require(
                isinstance(sample, dict),
                f"qualification measurement {metric} sample {sample_index} is malformed",
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
                    "process",
                    "server_identity",
                    "clock",
                    "start",
                    "end",
                    "operands",
                    "suspend_witness",
                },
                f"qualification measurement {metric} sample {sample_index}",
            )
            sample_id = require_opaque_identifier(
                sample["sample_id"],
                f"qualification measurement {metric} sample_id",
            )
            require(
                sample_id not in sample_ids,
                f"qualification measurement {metric} duplicated a sample id",
            )
            sample_ids.add(sample_id)
            require(
                sample["repeat"] == sample_index + 1,
                f"qualification measurement {metric} repeat sequence is not exact",
            )
            require(
                sample["matrix_cell_id"] == matrix_cell_id,
                f"qualification measurement {metric} used the wrong host/package matrix cell",
            )
            require(
                sample["workload_id"] == protocol["workloads"][metric]["workload_id"],
                f"qualification measurement {metric} used the wrong workload",
            )
            require(
                sample["cache_state"] == matrix_cell["cache_state"]
                and sample["residency_state"] == matrix_cell["residency_state"],
                f"qualification measurement {metric} changed cache or residency state",
            )
            server_identity = sample["server_identity"]
            require(
                isinstance(server_identity, dict),
                f"qualification measurement {metric} server identity is malformed",
            )
            require_exact_keys(
                server_identity,
                {"server_instance_id", "process_start_id", "load_generation"},
                f"qualification measurement {metric} server identity",
            )
            server_identities.append(
                (
                    require_opaque_identifier(
                        server_identity["server_instance_id"],
                        f"qualification measurement {metric} server_instance_id",
                    ),
                    require_nonempty_string(
                        server_identity["process_start_id"],
                        f"qualification measurement {metric} server process_start_id",
                    ),
                    require_positive_int(
                        server_identity["load_generation"],
                        f"qualification measurement {metric} server load_generation",
                    ),
                )
            )
            sample_values.append(
                qualification_measurement_sample_value(
                    metric,
                    sample,
                    contracts=contracts,
                    phase_boundaries=phase_boundaries,
                    allowed_awake_apis=allowed_awake_apis,
                    inclusive_api=inclusive_api,
                    maximum_suspend_ns=maximum_suspend_ns,
                    expected_policy=expected_policy,
                    expected_backend=expected_backend,
                )
            )
        if sample_policy.get("independence") == "distinct_server_instance_per_sample":
            require(
                len({identity[:2] for identity in server_identities}) == len(samples),
                f"qualification measurement {metric} repeats did not use distinct server instances",
            )
        else:
            require(
                len(set(server_identities)) == 1,
                f"qualification measurement {metric} changed server identity within its repeated block",
            )
        aggregation = sample_policy["aggregation"]
        values[metric] = {
            "maximum": max,
            "minimum": min,
            "exact": lambda raw: raw[0],
        }[aggregation](sample_values)

    require(
        summary["sample_count"] == sample_count,
        "qualification measurement sample count is stale",
    )
    return {
        "artifact": {
            "name": name,
            "sha256": hashlib.sha256(payload_bytes).hexdigest(),
        },
        "values": values,
        "unplanned_suspend": False,
        "matrix_cell_id": matrix_cell_id,
        "payload": payload,
    }
