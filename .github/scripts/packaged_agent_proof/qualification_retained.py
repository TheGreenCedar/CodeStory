"""Verification for retained qualification evidence."""

from __future__ import annotations

import re
from dataclasses import dataclass
from pathlib import Path

from .foundation import (
    CANDIDATE_PRODUCER_WORKFLOW_PATHS,
    LOWER_TIER_NONCLAIMS,
    MIN_RETRIEVAL_QUALITY_REPEATS,
    PINNED_CODEX_CLI_VERSION,
    QUALIFICATION_SCHEMA_VERSION,
    RELEASE_QUALITY_CORPUS_ID,
    REQUIRED_SERVER_SCENARIOS,
    RETRIEVAL_QUALITY_EVIDENCE_CONTRACT,
    require,
)
from .contracts import (
    require_exact_keys,
    require_nonempty_string,
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
