import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { mkdtempSync, readFileSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import test from "node:test";

import {
  buildRestartSurvivalReceipt,
  RECEIPT_SCHEMA,
} from "./restart-survival-receipt.mjs";
import {
  DEFERRED_INSTALLATION_SOURCE,
  LIVE_INSTALLATION_SOURCE,
  LIVE_MARKETPLACE_REPOSITORY,
} from "./marketplace-delivery-identity.mjs";

const SCRIPT = new URL("./restart-survival-receipt.mjs", import.meta.url).pathname;
const SOURCE = "1".repeat(40);
const TREE = "2".repeat(40);
const MARKETPLACE = "3".repeat(40);
const BINARY = "a".repeat(64);
const RUNTIME_BINARY = "b".repeat(64);
const ARCHIVE = "c".repeat(64);
const PACKAGE = "d".repeat(64);
const MODEL = "e".repeat(64);
const VERSION = "0.17.0";

function installedPlugin() {
  return {
    schema_version: 2,
    installation_source: LIVE_INSTALLATION_SOURCE,
    codex_cli_version: "codex-cli 1.2.3",
    marketplace_repository: LIVE_MARKETPLACE_REPOSITORY,
    marketplace_commit: MARKETPLACE,
    plugin_id: "codestory",
    plugin_version: VERSION,
    plugin_source_commit: SOURCE,
    plugin_source_tree: TREE,
    plugin_package_sha256: PACKAGE,
  };
}

function managedRuntime() {
  return {
    cli_source: "managed",
    plugin_version: VERSION,
    managed_binary_sha256: BINARY,
    archive_sha256: ARCHIVE,
    build_source: "github_release",
    repo_ref: `v${VERSION}`,
    provisioned_at: "2026-08-09T12:00:00Z",
  };
}

function summary(ordinal) {
  return {
    package_contract: {
      manifest: {
        schema_version: 3,
        release_version: VERSION,
        asset_target: "macos-arm64",
        source: { commit: SOURCE, tree: TREE, tracked_dirty: false },
        binary: { name: "codestory-cli", sha256: BINARY },
        runtime_executable: {
          name: "codestory-cli-native",
          sha256: RUNTIME_BINARY,
        },
        engine: { build_identity: "codestory-native-engine-v1|fixture|end" },
      },
      claim_scope: "server_behavior_only",
      highest_proof_tier: "installed_runtime",
      release_readiness_claim: true,
      runtime_evidence: {
        build_identity: "codestory-native-engine-v1|fixture|end",
        model_sha256: MODEL,
        policy: "accelerated",
        backend: "metal",
        execution: "proven_by_live_runtime",
      },
    },
    server_behavior: {
      status: "pass",
      runtime_tier_exercised: "installed_runtime",
      project_bound: true,
      retrieval_ready: true,
      release_readiness_claim: true,
      installed_runtime_provenance_proven: true,
    },
    runtime: {
      installed_plugin: installedPlugin(),
      managed_runtime: managedRuntime(),
      ground: {
        status: "pass",
        attempts: 1,
        project_bound: true,
        response_nonempty: true,
      },
      search: {
        status: "pass",
        attempts: 2,
        project_bound: true,
        retrieval_ready: true,
      },
      identity: {
        embedding_model_sha256: MODEL,
        embedding_ggml_build_identity: "codestory-native-engine-v1|fixture|end",
        embedding_backend: "Metal",
        embedding_adapter: "Apple GPU",
        embedding_policy: "accelerated",
        embedding_engine_instance_id: `engine-instance-${ordinal}`,
        embedding_accelerator_execution_verified: true,
        embedding_execution_observation_source: "ggml_eval_callback",
        embedding_execution_backends: ["Metal"],
        embedding_execution_devices: ["Apple GPU"],
        embedding_encode_count: ordinal,
        embedding_execution_node_count: 10,
      },
      snapshot: {
        process: {
          server_instance_id: `server-${ordinal}`,
          process_start_id: `boot:${100 + ordinal}`,
          executable_sha256: RUNTIME_BINARY,
          executable_version: VERSION,
        },
        engine: {
          engine_owner_id: `engine-owner-${ordinal}`,
          native_worker_id: `worker-${ordinal}`,
        },
      },
    },
  };
}

function attestation() {
  return {
    schema_version: 2,
    installation_source: LIVE_INSTALLATION_SOURCE,
    installation: {
      codex_home: "/isolated/codex-home",
      plugin_root: "/isolated/codex-home/plugins/codestory",
      plugin_data: "/isolated/codex-home/data/codestory",
    },
    plugin: {
      id: "codestory",
      version: VERSION,
      source_commit: SOURCE,
      source_tree: TREE,
      package_sha256: PACKAGE,
    },
    marketplace: {
      repository: LIVE_MARKETPLACE_REPOSITORY,
      revision: MARKETPLACE,
      codex_cli_version: "codex-cli 1.2.3",
    },
  };
}

function world(mutate = () => {}) {
  const directory = mkdtempSync(path.join(tmpdir(), "restart-survival-"));
  const state = {
    first: summary(1),
    second: summary(2),
    attestation: attestation(),
    options: {
      catalogDeliveryState: "published",
      expectedInstallerIdentity: LIVE_INSTALLATION_SOURCE,
      expectedSourceCommit: SOURCE,
      expectedSourceTree: TREE,
      expectedVersion: VERSION,
      expectedArchiveSha256: ARCHIVE,
      expectedBinarySha256: BINARY,
      expectedBackend: "metal",
      session1StartMs: 1_000,
      session1FinishedMs: 2_000,
      session2StartMs: 3_000,
      session2FinishedMs: 4_000,
    },
  };
  mutate(state);
  const firstPath = path.join(directory, "session-1.json");
  const secondPath = path.join(directory, "session-2.json");
  const attestationPath = path.join(directory, "install-attestation.json");
  writeFileSync(firstPath, `${JSON.stringify(state.first, null, 2)}\n`);
  writeFileSync(secondPath, `${JSON.stringify(state.second, null, 2)}\n`);
  writeFileSync(attestationPath, `${JSON.stringify(state.attestation, null, 2)}\n`);
  Object.assign(state.options, {
    session1SummaryPath: firstPath,
    session2SummaryPath: secondPath,
    installAttestationPath: attestationPath,
  });
  return { ...state, directory };
}

function cliArgs(state, output) {
  const options = state.options;
  return [
    "--session-1-summary", options.session1SummaryPath,
    "--session-2-summary", options.session2SummaryPath,
    "--install-attestation", options.installAttestationPath,
    "--catalog-delivery-state", options.catalogDeliveryState,
    "--expected-installer-identity", options.expectedInstallerIdentity,
    "--expected-source-commit", options.expectedSourceCommit,
    "--expected-source-tree", options.expectedSourceTree,
    "--expected-version", options.expectedVersion,
    "--expected-archive-sha256", options.expectedArchiveSha256,
    "--expected-binary-sha256", options.expectedBinarySha256,
    "--expected-backend", options.expectedBackend,
    "--session-1-start-ms", String(options.session1StartMs),
    "--session-1-finished-ms", String(options.session1FinishedMs),
    "--session-2-start-ms", String(options.session2StartMs),
    "--session-2-finished-ms", String(options.session2FinishedMs),
    "--out", output,
  ];
}

test("builds a hashed pass receipt for one install across two ordered sessions", () => {
  const state = world();
  const receipt = buildRestartSurvivalReceipt(state.options);
  assert.equal(receipt.schema, RECEIPT_SCHEMA);
  assert.equal(receipt.status, "pass");
  assert.equal(receipt.installation.reused, true);
  assert.match(receipt.installation.attestation_sha256, /^[0-9a-f]{64}$/u);
  assert.equal(receipt.sessions.length, 2);
  assert.notEqual(receipt.sessions[0].summary_sha256, receipt.sessions[1].summary_sha256);
  assert.equal(receipt.sessions[1].search.retrieval_mode, "full");
  assert.equal(receipt.release.accelerator_backend, "metal");
  assert.deepEqual(receipt.continuity, {
    sequential: true,
    single_install_reused: true,
    server_restarted: true,
    process_restarted: true,
    engine_restarted: true,
    session_2_retrieval_ready: true,
    session_2_retrieval_mode: "full",
    session_2_accelerated_backend_observed: true,
  });
});

test("CLI writes the reusable receipt through explicit paths and metadata", () => {
  const state = world();
  const output = path.join(state.directory, "receipt", "restart-survival.json");
  const stdout = execFileSync(process.execPath, [SCRIPT, ...cliArgs(state, output)], {
    encoding: "utf8",
  });
  assert.equal(stdout, `${RECEIPT_SCHEMA}: pass\n`);
  assert.equal(JSON.parse(readFileSync(output, "utf8")).schema, RECEIPT_SCHEMA);
});

const hostileCases = [
  ["catalog installer disagrees with state", state => {
    state.options.expectedInstallerIdentity = DEFERRED_INSTALLATION_SOURCE;
  }, /catalog installer identity/u],
  ["catalog repository disagrees with state", state => {
    state.attestation.marketplace.repository = "wrong/catalog";
  }, /marketplace\.repository/u],
  ["attestation source disagrees with release", state => {
    state.attestation.plugin.source_commit = "4".repeat(40);
  }, /attestation plugin\.source_commit/u],
  ["session source commit changes", state => {
    state.second.package_contract.manifest.source.commit = "4".repeat(40);
  }, /session 2 manifest source commit/u],
  ["session source tree changes", state => {
    state.second.package_contract.manifest.source.tree = "4".repeat(40);
  }, /session 2 manifest source tree/u],
  ["session version changes", state => {
    state.second.package_contract.manifest.release_version = "0.17.1";
  }, /session 2 release version/u],
  ["session archive changes", state => {
    state.second.runtime.managed_runtime.archive_sha256 = "4".repeat(64);
  }, /session 2 archive sha256/u],
  ["session managed binary changes", state => {
    state.second.runtime.managed_runtime.managed_binary_sha256 = "4".repeat(64);
  }, /session 2 managed binary sha256/u],
  ["installed plugin identity changes", state => {
    state.second.runtime.installed_plugin.plugin_package_sha256 = "4".repeat(64);
  }, /installed plugin identity changed/u],
  ["managed runtime was reprovisioned", state => {
    state.second.runtime.managed_runtime.provisioned_at = "2026-08-09T12:05:00Z";
  }, /managed runtime identity changed/u],
  ["install identity is incomplete", state => {
    state.attestation.installation.plugin_root = "";
  }, /installation\.plugin_root/u],
  ["server instance survived instead of restarting", state => {
    state.second.runtime.snapshot.process.server_instance_id = "server-1";
  }, /server instance identity must differ/u],
  ["process start identity survived instead of restarting", state => {
    state.second.runtime.snapshot.process.process_start_id = "boot:101";
  }, /process-start identity must differ/u],
  ["engine instance survived instead of restarting", state => {
    state.second.runtime.identity.embedding_engine_instance_id = "engine-instance-1";
  }, /embedding engine instance identity must differ/u],
  ["engine owner survived instead of restarting", state => {
    state.second.runtime.snapshot.engine.engine_owner_id = "engine-owner-1";
  }, /engine owner identity must differ/u],
  ["native worker survived instead of restarting", state => {
    state.second.runtime.snapshot.engine.native_worker_id = "worker-1";
  }, /native worker identity must differ/u],
  ["sessions overlap", state => {
    state.options.session2StartMs = state.options.session1FinishedMs;
  }, /session 1 must finish before session 2 begins/u],
  ["session 1 ground is not project-bound", state => {
    state.first.runtime.ground.project_bound = false;
  }, /session 1 runtime\.ground\.project_bound/u],
  ["session 1 search is not project-bound", state => {
    state.first.runtime.search.project_bound = false;
  }, /session 1 runtime\.search\.project_bound/u],
  ["session 2 ground is not project-bound", state => {
    state.second.runtime.ground.project_bound = false;
  }, /session 2 runtime\.ground\.project_bound/u],
  ["session 2 search is not project-bound", state => {
    state.second.runtime.search.project_bound = false;
  }, /session 2 runtime\.search\.project_bound/u],
  ["session 2 retrieval is not ready", state => {
    state.second.runtime.search.retrieval_ready = false;
  }, /session 2 runtime\.search\.retrieval_ready/u],
  ["session 2 summary does not claim installed provenance", state => {
    state.second.server_behavior.installed_runtime_provenance_proven = false;
  }, /installed_runtime_provenance_proven/u],
  ["session 2 uses a non-accelerated policy", state => {
    state.second.runtime.identity.embedding_policy = "cpu_explicit";
  }, /session 2 embedding policy/u],
  ["session 2 lacks live accelerator observation", state => {
    state.second.runtime.identity.embedding_accelerator_execution_verified = false;
  }, /session 2 accelerator execution verification/u],
  ["session 2 observes the wrong backend", state => {
    state.second.runtime.identity.embedding_backend = "Vulkan";
  }, /session 2 embedding backend/u],
  ["server executable differs from the package", state => {
    state.second.runtime.snapshot.process.executable_sha256 = "4".repeat(64);
  }, /session 2 process executable sha256/u],
];

test("fails closed for every hostile restart-survival mismatch", async t => {
  for (const [name, mutate, expectedError] of hostileCases) {
    await t.test(name, () => {
      const state = world(mutate);
      assert.throws(
        () => buildRestartSurvivalReceipt(state.options),
        expectedError,
      );
    });
  }
});

test("CLI rejects missing and unknown metadata instead of defaulting", () => {
  const missing = world();
  assert.throws(
    () => execFileSync(process.execPath, [SCRIPT, ...cliArgs(missing, "unused").slice(2)], {
      encoding: "utf8",
      stdio: "pipe",
    }),
    /Command failed/u,
  );

  const unknown = world();
  assert.throws(
    () => execFileSync(process.execPath, [
      SCRIPT,
      ...cliArgs(unknown, path.join(unknown.directory, "receipt.json")),
      "--allow-mismatch",
      "true",
    ], { encoding: "utf8", stdio: "pipe" }),
    /Command failed/u,
  );
});
