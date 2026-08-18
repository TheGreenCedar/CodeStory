#!/usr/bin/env node

import { createHash } from "node:crypto";
import { mkdirSync, readFileSync, renameSync, writeFileSync } from "node:fs";
import path from "node:path";
import process from "node:process";
import { pathToFileURL } from "node:url";

import {
  DEFERRED_INSTALLATION_SOURCE,
  DEFERRED_MARKETPLACE_REPOSITORY,
  LIVE_INSTALLATION_SOURCE,
  LIVE_MARKETPLACE_REPOSITORY,
  RESTORED_INSTALLATION_SOURCE,
  RESTORED_MARKETPLACE_REPOSITORY,
} from "./marketplace-delivery-identity.mjs";

export const RECEIPT_SCHEMA =
  "codestory.catalog-installed-restart-survival/v1";

const DELIVERY_IDENTITIES = Object.freeze({
  published: Object.freeze({
    installer: LIVE_INSTALLATION_SOURCE,
    repository: LIVE_MARKETPLACE_REPOSITORY,
  }),
  deferred: Object.freeze({
    installer: DEFERRED_INSTALLATION_SOURCE,
    repository: DEFERRED_MARKETPLACE_REPOSITORY,
  }),
  restored: Object.freeze({
    installer: RESTORED_INSTALLATION_SOURCE,
    repository: RESTORED_MARKETPLACE_REPOSITORY,
  }),
});

const CLI_KEYS = new Set([
  "session_1_summary",
  "session_2_summary",
  "install_attestation",
  "catalog_delivery_state",
  "expected_installer_identity",
  "expected_source_commit",
  "expected_source_tree",
  "expected_version",
  "expected_archive_sha256",
  "expected_binary_sha256",
  "expected_backend",
  "session_1_start_ms",
  "session_1_finished_ms",
  "session_2_start_ms",
  "session_2_finished_ms",
  "out",
]);

function fail(message) {
  throw new Error(message);
}

function object(value, label) {
  if (!value || typeof value !== "object" || Array.isArray(value)) {
    fail(`${label} must be an object`);
  }
  return value;
}

function nonEmpty(value, label) {
  if (typeof value !== "string" || value.trim().length === 0) {
    fail(`${label} must be a non-empty string`);
  }
  return value;
}

function exact(value, expected, label) {
  if (value !== expected) {
    fail(`${label} must be ${JSON.stringify(expected)}, got ${JSON.stringify(value)}`);
  }
  return value;
}

function commit(value, label) {
  if (typeof value !== "string" || !/^[0-9a-f]{40}$/u.test(value)) {
    fail(`${label} must be a full lowercase Git SHA`);
  }
  return value;
}

function digest(value, label) {
  if (typeof value !== "string" || !/^[0-9a-f]{64}$/u.test(value)) {
    fail(`${label} must be a lowercase SHA-256 digest`);
  }
  return value;
}

function positiveInteger(value, label) {
  if (!Number.isSafeInteger(value) || value <= 0) {
    fail(`${label} must be a positive safe integer`);
  }
  return value;
}

function parsePositiveInteger(value, label) {
  if (typeof value !== "string" || !/^[1-9][0-9]*$/u.test(value)) {
    fail(`${label} must be a positive integer`);
  }
  return positiveInteger(Number(value), label);
}

function sha256(value) {
  return createHash("sha256").update(value).digest("hex");
}

function sortJson(value) {
  if (Array.isArray(value)) return value.map(sortJson);
  if (value && typeof value === "object") {
    return Object.fromEntries(
      Object.keys(value)
        .sort()
        .map(key => [key, sortJson(value[key])]),
    );
  }
  return value;
}

function identityDigest(value) {
  return sha256(JSON.stringify(sortJson(value)));
}

function sameIdentity(left, right, label) {
  if (identityDigest(left) !== identityDigest(right)) {
    fail(`${label} changed between sessions`);
  }
}

function different(left, right, label) {
  nonEmpty(left, `session 1 ${label}`);
  nonEmpty(right, `session 2 ${label}`);
  if (left === right) {
    fail(`${label} must differ between sessions`);
  }
}

function readJsonEvidence(file, label) {
  let raw;
  try {
    raw = readFileSync(file);
  } catch (error) {
    fail(`${label} could not be read: ${error.message}`);
  }
  let value;
  try {
    value = JSON.parse(raw.toString("utf8"));
  } catch (error) {
    fail(`${label} is not valid JSON: ${error.message}`);
  }
  return { raw, value: object(value, label), sha256: sha256(raw) };
}

function normalizedBackend(value) {
  const normalized = nonEmpty(value, "accelerator backend").toLowerCase();
  return normalized === "mtl" ? "metal" : normalized;
}

function validateAttestation(attestation, expected) {
  exact(attestation.schema_version, 2, "install attestation schema_version");
  exact(
    attestation.installation_source,
    expected.delivery.installer,
    "install attestation installation_source",
  );

  const installation = object(
    attestation.installation,
    "install attestation installation",
  );
  for (const field of ["codex_home", "plugin_root", "plugin_data"]) {
    nonEmpty(installation[field], `install attestation installation.${field}`);
  }

  const plugin = object(attestation.plugin, "install attestation plugin");
  exact(plugin.id, "codestory", "install attestation plugin.id");
  exact(plugin.version, expected.version, "install attestation plugin.version");
  exact(
    commit(plugin.source_commit, "install attestation plugin.source_commit"),
    expected.sourceCommit,
    "install attestation plugin.source_commit",
  );
  exact(
    commit(plugin.source_tree, "install attestation plugin.source_tree"),
    expected.sourceTree,
    "install attestation plugin.source_tree",
  );
  digest(plugin.package_sha256, "install attestation plugin.package_sha256");

  const marketplace = object(
    attestation.marketplace,
    "install attestation marketplace",
  );
  exact(
    marketplace.repository,
    expected.delivery.repository,
    "install attestation marketplace.repository",
  );
  commit(marketplace.revision, "install attestation marketplace.revision");
  nonEmpty(
    marketplace.codex_cli_version,
    "install attestation marketplace.codex_cli_version",
  );

  return { installation, plugin, marketplace };
}

function validateProjectCalls(runtime, serverBehavior, label) {
  const ground = object(runtime.ground, `${label} runtime.ground`);
  exact(ground.status, "pass", `${label} runtime.ground.status`);
  exact(ground.project_bound, true, `${label} runtime.ground.project_bound`);
  exact(
    ground.response_nonempty,
    true,
    `${label} runtime.ground.response_nonempty`,
  );
  positiveInteger(ground.attempts, `${label} runtime.ground.attempts`);

  const search = object(runtime.search, `${label} runtime.search`);
  exact(search.status, "pass", `${label} runtime.search.status`);
  exact(search.project_bound, true, `${label} runtime.search.project_bound`);
  exact(search.retrieval_ready, true, `${label} runtime.search.retrieval_ready`);
  positiveInteger(search.attempts, `${label} runtime.search.attempts`);

  exact(serverBehavior.status, "pass", `${label} server_behavior.status`);
  exact(
    serverBehavior.runtime_tier_exercised,
    "installed_runtime",
    `${label} server_behavior.runtime_tier_exercised`,
  );
  exact(
    serverBehavior.project_bound,
    true,
    `${label} server_behavior.project_bound`,
  );
  exact(
    serverBehavior.retrieval_ready,
    true,
    `${label} server_behavior.retrieval_ready`,
  );
  exact(
    serverBehavior.installed_runtime_provenance_proven,
    true,
    `${label} server_behavior.installed_runtime_provenance_proven`,
  );
  exact(
    serverBehavior.release_readiness_claim,
    true,
    `${label} server_behavior.release_readiness_claim`,
  );
  return { ground, search };
}

function validateAccelerator(summary, runtime, expectedBackend, label) {
  const runtimeEvidence = object(
    summary.package_contract.runtime_evidence,
    `${label} package_contract.runtime_evidence`,
  );
  exact(
    runtimeEvidence.policy,
    "accelerated",
    `${label} runtime evidence policy`,
  );
  exact(
    runtimeEvidence.execution,
    "proven_by_live_runtime",
    `${label} runtime evidence execution`,
  );
  exact(
    normalizedBackend(runtimeEvidence.backend),
    expectedBackend,
    `${label} runtime evidence backend`,
  );

  const identity = object(runtime.identity, `${label} runtime.identity`);
  exact(
    identity.embedding_policy,
    "accelerated",
    `${label} embedding policy`,
  );
  exact(
    normalizedBackend(identity.embedding_backend),
    expectedBackend,
    `${label} embedding backend`,
  );
  exact(
    identity.embedding_accelerator_execution_verified,
    true,
    `${label} accelerator execution verification`,
  );
  exact(
    identity.embedding_execution_observation_source,
    "ggml_eval_callback",
    `${label} accelerator observation source`,
  );
  const observedBackends = identity.embedding_execution_backends;
  if (
    !Array.isArray(observedBackends)
    || !observedBackends.some(value =>
      typeof value === "string"
      && normalizedBackend(value) === expectedBackend)
  ) {
    fail(`${label} did not observe the expected accelerated backend`);
  }
  if (
    !Array.isArray(identity.embedding_execution_devices)
    || identity.embedding_execution_devices.length === 0
  ) {
    fail(`${label} did not observe an accelerator execution device`);
  }
  positiveInteger(
    identity.embedding_encode_count,
    `${label} embedding_encode_count`,
  );
  positiveInteger(
    identity.embedding_execution_node_count,
    `${label} embedding_execution_node_count`,
  );
  return { identity, runtimeEvidence };
}

function validateSummary(summary, expected, label) {
  const packageContract = object(
    summary.package_contract,
    `${label} package_contract`,
  );
  exact(
    packageContract.claim_scope,
    "server_behavior_only",
    `${label} package_contract.claim_scope`,
  );
  exact(
    packageContract.highest_proof_tier,
    "installed_runtime",
    `${label} package_contract.highest_proof_tier`,
  );
  exact(
    packageContract.release_readiness_claim,
    true,
    `${label} package_contract.release_readiness_claim`,
  );

  const manifest = object(packageContract.manifest, `${label} native manifest`);
  exact(manifest.release_version, expected.version, `${label} release version`);
  const source = object(manifest.source, `${label} manifest source`);
  exact(
    commit(source.commit, `${label} manifest source commit`),
    expected.sourceCommit,
    `${label} manifest source commit`,
  );
  exact(
    commit(source.tree, `${label} manifest source tree`),
    expected.sourceTree,
    `${label} manifest source tree`,
  );
  exact(source.tracked_dirty, false, `${label} manifest tracked_dirty`);
  const binary = object(manifest.binary, `${label} manifest binary`);
  exact(
    digest(binary.sha256, `${label} manifest binary sha256`),
    expected.binarySha256,
    `${label} manifest binary sha256`,
  );
  const runtimeExecutable = object(
    manifest.runtime_executable,
    `${label} manifest runtime_executable`,
  );
  digest(
    runtimeExecutable.sha256,
    `${label} manifest runtime_executable sha256`,
  );

  const runtime = object(summary.runtime, `${label} runtime`);
  const installedPlugin = object(
    runtime.installed_plugin,
    `${label} runtime.installed_plugin`,
  );
  exact(installedPlugin.schema_version, 2, `${label} installed plugin schema`);
  exact(
    installedPlugin.installation_source,
    expected.delivery.installer,
    `${label} installed plugin installation_source`,
  );
  exact(installedPlugin.plugin_id, "codestory", `${label} installed plugin id`);
  exact(
    installedPlugin.plugin_version,
    expected.version,
    `${label} installed plugin version`,
  );
  exact(
    commit(installedPlugin.plugin_source_commit, `${label} plugin source commit`),
    expected.sourceCommit,
    `${label} plugin source commit`,
  );
  exact(
    commit(installedPlugin.plugin_source_tree, `${label} plugin source tree`),
    expected.sourceTree,
    `${label} plugin source tree`,
  );
  digest(installedPlugin.plugin_package_sha256, `${label} plugin package sha256`);
  exact(
    installedPlugin.marketplace_repository,
    expected.delivery.repository,
    `${label} installed plugin marketplace_repository`,
  );
  commit(installedPlugin.marketplace_commit, `${label} marketplace commit`);
  nonEmpty(installedPlugin.codex_cli_version, `${label} Codex CLI version`);

  const managedRuntime = object(
    runtime.managed_runtime,
    `${label} runtime.managed_runtime`,
  );
  exact(managedRuntime.cli_source, "managed", `${label} managed cli_source`);
  exact(
    managedRuntime.plugin_version,
    expected.version,
    `${label} managed plugin_version`,
  );
  exact(
    digest(managedRuntime.managed_binary_sha256, `${label} managed binary sha256`),
    expected.binarySha256,
    `${label} managed binary sha256`,
  );
  exact(
    digest(managedRuntime.archive_sha256, `${label} archive sha256`),
    expected.archiveSha256,
    `${label} archive sha256`,
  );
  nonEmpty(managedRuntime.build_source, `${label} managed build_source`);
  nonEmpty(managedRuntime.repo_ref, `${label} managed repo_ref`);
  nonEmpty(managedRuntime.provisioned_at, `${label} managed provisioned_at`);

  const serverBehavior = object(summary.server_behavior, `${label} server_behavior`);
  const calls = validateProjectCalls(runtime, serverBehavior, label);
  const accelerator = validateAccelerator(
    summary,
    runtime,
    expected.backend,
    label,
  );

  const snapshot = object(runtime.snapshot, `${label} runtime.snapshot`);
  const processIdentity = object(
    snapshot.process,
    `${label} runtime.snapshot.process`,
  );
  nonEmpty(processIdentity.server_instance_id, `${label} server instance id`);
  nonEmpty(processIdentity.process_start_id, `${label} process start id`);
  exact(
    digest(processIdentity.executable_sha256, `${label} process executable sha256`),
    runtimeExecutable.sha256,
    `${label} process executable sha256`,
  );
  exact(
    processIdentity.executable_version,
    expected.version,
    `${label} process executable version`,
  );

  const snapshotEngine = object(snapshot.engine, `${label} runtime.snapshot.engine`);
  nonEmpty(snapshotEngine.engine_owner_id, `${label} engine owner id`);
  nonEmpty(snapshotEngine.native_worker_id, `${label} native worker id`);
  nonEmpty(
    accelerator.identity.embedding_engine_instance_id,
    `${label} embedding engine instance id`,
  );

  return {
    manifest,
    installedPlugin,
    managedRuntime,
    runtimeEvidence: accelerator.runtimeEvidence,
    identity: accelerator.identity,
    processIdentity,
    snapshotEngine,
    ground: calls.ground,
    search: calls.search,
  };
}

function validateCrossSessionIdentity(first, second, attestation, expected) {
  sameIdentity(first.manifest, second.manifest, "native package manifest");
  sameIdentity(
    first.installedPlugin,
    second.installedPlugin,
    "installed plugin identity",
  );
  sameIdentity(
    first.managedRuntime,
    second.managedRuntime,
    "managed runtime identity",
  );

  for (const session of [first, second]) {
    exact(
      session.installedPlugin.plugin_package_sha256,
      attestation.plugin.package_sha256,
      "installed plugin package identity",
    );
    exact(
      session.installedPlugin.marketplace_commit,
      attestation.marketplace.revision,
      "installed catalog revision",
    );
    exact(
      session.installedPlugin.codex_cli_version,
      attestation.marketplace.codex_cli_version,
      "installed Codex CLI version",
    );
    exact(
      session.managedRuntime.repo_ref,
      `v${expected.version}`,
      "managed runtime release ref",
    );
    exact(
      session.managedRuntime.build_source,
      "github_release",
      "managed runtime build source",
    );
  }

  for (const field of [
    "embedding_model_sha256",
    "embedding_ggml_build_identity",
    "embedding_backend",
    "embedding_adapter",
    "embedding_policy",
  ]) {
    exact(
      second.identity[field],
      first.identity[field],
      `stable engine identity ${field}`,
    );
  }
  sameIdentity(first.runtimeEvidence, second.runtimeEvidence, "runtime accelerator identity");

  different(
    first.processIdentity.server_instance_id,
    second.processIdentity.server_instance_id,
    "server instance identity",
  );
  different(
    first.processIdentity.process_start_id,
    second.processIdentity.process_start_id,
    "process-start identity",
  );
  different(
    first.identity.embedding_engine_instance_id,
    second.identity.embedding_engine_instance_id,
    "embedding engine instance identity",
  );
  different(
    first.snapshotEngine.engine_owner_id,
    second.snapshotEngine.engine_owner_id,
    "engine owner identity",
  );
  different(
    first.snapshotEngine.native_worker_id,
    second.snapshotEngine.native_worker_id,
    "native worker identity",
  );
}

function validateTimings(timings) {
  for (const [key, value] of Object.entries(timings)) {
    positiveInteger(value, key);
  }
  if (timings.session1StartMs >= timings.session1FinishedMs) {
    fail("session 1 must finish after it starts");
  }
  if (timings.session2StartMs >= timings.session2FinishedMs) {
    fail("session 2 must finish after it starts");
  }
  if (timings.session1FinishedMs >= timings.session2StartMs) {
    fail("session 1 must finish before session 2 begins");
  }
}

export function buildRestartSurvivalReceipt(options) {
  const delivery = DELIVERY_IDENTITIES[options.catalogDeliveryState];
  if (!delivery) {
    fail(
      `catalog delivery state must be one of ${Object.keys(DELIVERY_IDENTITIES).join(", ")}`,
    );
  }
  exact(
    options.expectedInstallerIdentity,
    delivery.installer,
    "catalog installer identity",
  );
  const expected = {
    delivery,
    sourceCommit: commit(options.expectedSourceCommit, "expected source commit"),
    sourceTree: commit(options.expectedSourceTree, "expected source tree"),
    version: nonEmpty(options.expectedVersion, "expected version"),
    archiveSha256: digest(
      options.expectedArchiveSha256,
      "expected archive sha256",
    ),
    binarySha256: digest(
      options.expectedBinarySha256,
      "expected binary sha256",
    ),
    backend: normalizedBackend(options.expectedBackend),
  };
  const timings = {
    session1StartMs: options.session1StartMs,
    session1FinishedMs: options.session1FinishedMs,
    session2StartMs: options.session2StartMs,
    session2FinishedMs: options.session2FinishedMs,
  };
  validateTimings(timings);

  const firstEvidence = readJsonEvidence(
    options.session1SummaryPath,
    "session 1 summary",
  );
  const secondEvidence = readJsonEvidence(
    options.session2SummaryPath,
    "session 2 summary",
  );
  const attestationEvidence = readJsonEvidence(
    options.installAttestationPath,
    "install attestation",
  );
  const attestation = validateAttestation(attestationEvidence.value, expected);
  const first = validateSummary(firstEvidence.value, expected, "session 1");
  const second = validateSummary(secondEvidence.value, expected, "session 2");
  validateCrossSessionIdentity(first, second, attestation, expected);

  const sessionReceipt = (ordinal, evidence, session, startedAt, finishedAt) => ({
    ordinal,
    summary_sha256: evidence.sha256,
    started_at_epoch_ms: startedAt,
    finished_at_epoch_ms: finishedAt,
    ground: {
      status: "pass",
      project_bound: true,
      attempts: session.ground.attempts,
    },
    search: {
      status: "pass",
      project_bound: true,
      retrieval_state: "ready",
      retrieval_mode: "full",
      attempts: session.search.attempts,
    },
    server_instance_id: session.processIdentity.server_instance_id,
    process_start_id: session.processIdentity.process_start_id,
    engine_instance_id: session.identity.embedding_engine_instance_id,
    engine_owner_id: session.snapshotEngine.engine_owner_id,
    native_worker_id: session.snapshotEngine.native_worker_id,
  });

  return {
    schema: RECEIPT_SCHEMA,
    status: "pass",
    catalog: {
      delivery_state: options.catalogDeliveryState,
      installer_identity: delivery.installer,
      marketplace_repository: delivery.repository,
      marketplace_revision: attestation.marketplace.revision,
    },
    release: {
      version: expected.version,
      source_commit: expected.sourceCommit,
      source_tree: expected.sourceTree,
      archive_sha256: expected.archiveSha256,
      managed_binary_sha256: expected.binarySha256,
      runtime_executable_sha256: first.manifest.runtime_executable.sha256,
      accelerator_backend: expected.backend,
    },
    installation: {
      attestation_sha256: attestationEvidence.sha256,
      installation_identity_sha256: identityDigest(attestation.installation),
      plugin_package_sha256: attestation.plugin.package_sha256,
      installed_plugin_identity_sha256: identityDigest(first.installedPlugin),
      managed_runtime_identity_sha256: identityDigest(first.managedRuntime),
      reused: true,
    },
    sessions: [
      sessionReceipt(
        1,
        firstEvidence,
        first,
        timings.session1StartMs,
        timings.session1FinishedMs,
      ),
      sessionReceipt(
        2,
        secondEvidence,
        second,
        timings.session2StartMs,
        timings.session2FinishedMs,
      ),
    ],
    continuity: {
      sequential: true,
      single_install_reused: true,
      server_restarted: true,
      process_restarted: true,
      engine_restarted: true,
      session_2_retrieval_ready: true,
      session_2_retrieval_mode: "full",
      session_2_accelerated_backend_observed: true,
    },
  };
}

function parseArgs(argv) {
  if (argv.length % 2 !== 0) fail("arguments must be --name value pairs");
  const values = {};
  for (let index = 0; index < argv.length; index += 2) {
    const flag = argv[index];
    const value = argv[index + 1];
    if (!flag.startsWith("--")) fail(`invalid argument ${flag}`);
    const key = flag.slice(2).replaceAll("-", "_");
    if (!CLI_KEYS.has(key)) fail(`unknown argument ${flag}`);
    if (Object.hasOwn(values, key)) fail(`duplicate argument ${flag}`);
    values[key] = value;
  }
  for (const key of CLI_KEYS) {
    if (!Object.hasOwn(values, key) || values[key].length === 0) {
      fail(`--${key.replaceAll("_", "-")} is required`);
    }
  }
  return values;
}

function writeReceipt(file, receipt) {
  const output = path.resolve(file);
  mkdirSync(path.dirname(output), { recursive: true });
  const temporary = `${output}.${process.pid}.tmp`;
  writeFileSync(temporary, `${JSON.stringify(receipt, null, 2)}\n`, {
    encoding: "utf8",
    flag: "wx",
  });
  renameSync(temporary, output);
}

export function main(argv) {
  const args = parseArgs(argv);
  const receipt = buildRestartSurvivalReceipt({
    session1SummaryPath: args.session_1_summary,
    session2SummaryPath: args.session_2_summary,
    installAttestationPath: args.install_attestation,
    catalogDeliveryState: args.catalog_delivery_state,
    expectedInstallerIdentity: args.expected_installer_identity,
    expectedSourceCommit: args.expected_source_commit,
    expectedSourceTree: args.expected_source_tree,
    expectedVersion: args.expected_version,
    expectedArchiveSha256: args.expected_archive_sha256,
    expectedBinarySha256: args.expected_binary_sha256,
    expectedBackend: args.expected_backend,
    session1StartMs: parsePositiveInteger(
      args.session_1_start_ms,
      "session 1 start ms",
    ),
    session1FinishedMs: parsePositiveInteger(
      args.session_1_finished_ms,
      "session 1 finished ms",
    ),
    session2StartMs: parsePositiveInteger(
      args.session_2_start_ms,
      "session 2 start ms",
    ),
    session2FinishedMs: parsePositiveInteger(
      args.session_2_finished_ms,
      "session 2 finished ms",
    ),
  });
  writeReceipt(args.out, receipt);
  process.stdout.write(`${RECEIPT_SCHEMA}: pass\n`);
  return receipt;
}

if (
  process.argv[1]
  && import.meta.url === pathToFileURL(path.resolve(process.argv[1])).href
) {
  try {
    main(process.argv.slice(2));
  } catch (error) {
    process.stderr.write(`restart-survival receipt failed: ${error.message}\n`);
    process.exitCode = 1;
  }
}
