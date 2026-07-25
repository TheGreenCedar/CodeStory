#!/usr/bin/env node
import { createHash } from "node:crypto";
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { LineCounter, parseDocument } from "yaml";
import { loadReleaseClaimGraph } from "../../scripts/codestory-release-claims.mjs";

const workflowRoot = path.join(".github", "workflows");
const retrievalFile = "retrieval-engine-smoke.yml";
const repositoryRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "../..");
const trustedActionOwners = new Set(["actions", "github"]);
const fullSha = /^[0-9a-f]{40}$/iu;
const sccacheAction = "mozilla-actions/sccache-action@9e7fa8a12102821edf02ca5dbea1acd0f89a2696";
const sccacheVersion = "v0.16.0";
const nextestVersion = "0.9.98";
const nextestLinuxSha256 = "7d07712519615722b19ffe3b3d1097b7d4fa390995e3cac1f9d6dda1ba61b2a7";
const sccacheCacheSize = "1G";
const windowsSccacheCacheSize = "2G";

export { retrievalFile };

function object(value) {
  return value !== null && typeof value === "object" && !Array.isArray(value) ? value : {};
}

function list(value) {
  if (value === undefined || value === null) return [];
  return Array.isArray(value) ? value : [value];
}

function at(value, ...keys) {
  let current = value;
  for (const key of keys) {
    if (current === null || typeof current !== "object") return undefined;
    current = current[key];
  }
  return current;
}

function scalarStrings(value, found = []) {
  if (typeof value === "string") {
    found.push(value);
  } else if (Array.isArray(value)) {
    for (const item of value) scalarStrings(item, found);
  } else if (value !== null && typeof value === "object") {
    for (const item of Object.values(value)) scalarStrings(item, found);
  }
  return found;
}

function includesAll(values, expected) {
  const present = new Set(list(values));
  return expected.every(value => present.has(value));
}

function sameMembers(actual, expected) {
  const left = [...new Set(list(actual))].sort();
  const right = [...new Set(expected)].sort();
  return JSON.stringify(left) === JSON.stringify(right);
}

function needs(job) {
  return list(job?.needs);
}

function namedStep(job, name) {
  const matches = list(job?.steps).filter(step => object(step).name === name);
  return matches.length === 1 ? matches[0] : undefined;
}

function stepRun(job, name) {
  const run = namedStep(job, name)?.run;
  return typeof run === "string" ? run : "";
}

function executableRunText(run) {
  return run
    .split(/\r?\n/u)
    .filter(line => !/^\s*#/u.test(line))
    .join("\n");
}

function add(violations, condition, message) {
  if (!condition) violations.push(message);
}

function requireStepRun(violations, file, job, name, fragments) {
  const run = executableRunText(stepRun(job, name));
  add(violations, run.length > 0, `${file} must contain named step ${name}`);
  for (const fragment of fragments) {
    add(
      violations,
      run.includes(fragment),
      `${file} step ${name} must run ${fragment}`,
    );
  }
}

function forbidStepRun(violations, file, job, name, fragments) {
  const run = executableRunText(stepRun(job, name));
  for (const fragment of fragments) {
    add(
      violations,
      !run.includes(fragment),
      `${file} step ${name} must not run ${fragment}`,
    );
  }
}

function occurrenceCount(value, fragment) {
  return value.split(fragment).length - 1;
}

function requireNoCalibrationReferences(violations, file, workflow) {
  add(
    violations,
    !JSON.stringify(workflow).toLowerCase().includes("calibration"),
    `${file} standard release path must not reference calibration`,
  );
}

function requireOptionalStringInput(violations, file, workflow, event, key) {
  const input = object(at(workflow, "on", event, "inputs", key));
  add(
    violations,
    input.required === false && input.type === "string" && input.default === "",
    `${file} ${event} ${key} must be an optional empty string`,
  );
}

function exactResolverRunText(run) {
  return run.replace(/\r\n/gu, "\n");
}

function requireExactResolverContract(violations, file, job, expectedDigest) {
  const run = exactResolverRunText(stepRun(job, "Resolve trusted exact head"));
  const digest = createHash("sha256").update(run).digest("hex");
  add(
    violations,
    run.length > 0 && digest === expectedDigest,
    `${file} resolver must match the exact normalized trusted resolver script contract`,
  );
}

function stepIndex(job, name) {
  return list(job?.steps).map(object).findIndex(step => step.name === name);
}

function cacheSteps(job) {
  return list(job?.steps)
    .map(object)
    .filter(step => typeof step.uses === "string"
      && /^actions\/cache\/(?:restore|save)@/iu.test(step.uses));
}

function cachePaths(step) {
  return String(object(step?.with).path ?? "")
    .split(/\r?\n/u)
    .map(value => value.trim())
    .filter(Boolean);
}

function cachePathsExcludeExactOutputs(job) {
  const forbidden = /(^|[/\\])target($|[/\\])|release-dist|native-seeds|embedded-model|notarization|qualification|proof|\.tar\.gz$|\.zip$|sha256/iu;
  return cacheSteps(job).every(step =>
    cachePaths(step).every(cachePath => !forbidden.test(cachePath)));
}

function requireStepUses(violations, file, job, name, expected) {
  add(
    violations,
    namedStep(job, name)?.uses === expected,
    `${file} step ${name} must use ${expected}`,
  );
}

function requireCalibrationProducerAuthentication(violations, file, job) {
  requireStepRun(violations, file, job, "Authenticate calibration bundle producer", [
    "actions/runs/",
    ".github/workflows/packaged-platform-pr.yml",
    "workflow_dispatch",
    "success",
    "embedding-calibration-bundle-",
    "artifacts?per_page=100",
    "expired",
  ]);
}

function requireCalibrationProducerBoundary(
  violations,
  file,
  job,
  expectedCondition,
) {
  requireCalibrationProducerAuthentication(violations, file, job);
  requireStepUses(
    violations,
    file,
    job,
    "Download frozen calibration bundle",
    "actions/download-artifact@v8.0.1",
  );
  const authentication = namedStep(job, "Authenticate calibration bundle producer");
  const download = namedStep(job, "Download frozen calibration bundle");
  add(
    violations,
    authentication?.if === expectedCondition && download?.if === expectedCondition,
    `${file} calibration authentication and download must run only for qualification or freeze lineage`,
  );
  add(
    violations,
    object(download?.with)["run-id"] === "${{ inputs.calibration_bundle_run_id }}"
      && object(download?.with).name === "${{ inputs.calibration_bundle_artifact }}"
      && object(download?.with)["github-token"] === "${{ github.token }}",
    `${file} frozen calibration download must bind its artifact name, prior run, and token`,
  );
}

function requireJob(violations, file, workflow, name) {
  const found = object(workflow.jobs)[name];
  add(violations, found !== undefined, `${file} must contain job ${name}`);
  return object(found);
}

const draftCachePaths = [
  "~/.cargo/registry",
  "~/.cargo/git",
  "target",
];
const sourceResolverContractDigest = "2fe869b675010f5db29259aff38d83456c01dbc9885989afbf7c92a2826791af";
const platformResolverContractDigest = "12f5e887eb236625eec5e9718edd305ba625ab06f9a1467ed1146a8a80db0f74";
const draftProofCommands = [
  "cargo test --locked -p codestory-llama-sys --test native_staging",
  "cargo test --locked -p codestory-llama-sys --test model_staging",
  "cargo test --locked -p codestory-cli --test native_launcher_contracts",
  "cargo test --locked -p codestory-cli --test stdio_protocol_contracts two_stdio_processes_observe_only_complete_generations_during_real_refresh -- --nocapture",
  "cargo test --locked -p codestory-runtime publication_transitions_fail_or_cancel_atomically -- --nocapture",
  "cargo test --locked -p codestory-store staged_promotion_abort_recovers_old_or_complete_new_and_cleans_artifacts -- --nocapture",
];
const draftSeedCommands = [
  "cargo test --locked -p codestory-llama-sys --test native_staging --no-run",
  "cargo test --locked -p codestory-llama-sys --test model_staging --no-run",
  "cargo test --locked -p codestory-cli --test stdio_protocol_contracts --no-run two_stdio_processes_observe_only_complete_generations_during_real_refresh -- --nocapture",
  "cargo test --locked -p codestory-runtime --no-run publication_transitions_fail_or_cancel_atomically -- --nocapture",
  "cargo test --locked -p codestory-store --no-run staged_promotion_abort_recovers_old_or_complete_new_and_cleans_artifacts -- --nocapture",
];
const draftProofTopologyDigest = createHash("sha256")
  .update(draftSeedCommands.join("\n"))
  .digest("hex");
const draftProofTopology = `proof5-v1-${draftProofTopologyDigest}`;
const cacheRunner = "${{ runner.os }}";
const cacheRustVersion = "${{ steps.rust-cache-key.outputs.version }}";
const cacheTarget = "${{ steps.rust-cache-key.outputs.target }}";
const cacheManifests = "${{ hashFiles('Cargo.toml', 'crates/**/Cargo.toml', 'vendor/**/Cargo.toml') }}";
const cacheLock = "${{ hashFiles('Cargo.lock') }}";
const draftCachePrefix = [
  cacheRunner,
  "draft-v2",
  cacheRustVersion,
  cacheTarget,
  "workspace",
  draftProofTopology,
  "default-features",
  cacheManifests,
].join("-");
const retrievalCachePrefix = [
  cacheRunner,
  "cargo-stable",
  cacheRustVersion,
  cacheTarget,
  "retrieval-contracts",
  draftProofTopology,
  "default-features",
  cacheManifests,
].join("-");
const draftCachePrimary = `${draftCachePrefix}-${cacheLock}`;
const retrievalCachePrimary = `${retrievalCachePrefix}-${cacheLock}`;
const draftCacheRestoreKeys = [
  retrievalCachePrimary,
  `${draftCachePrefix}-`,
  `${retrievalCachePrefix}-`,
];
const cacheSaveCondition = "success() && steps.cargo-cache-restore.outputs.cache-hit != 'true' && steps.cargo-cache-restore.outputs.cache-primary-key != ''";
const draftCompilerCachePath = "${{ runner.temp }}/codestory-draft-sccache";
const draftCompilerCachePrefix =
  "${{ runner.os }}-draft-sccache-v1-${{ steps.rust-cache-key.outputs.version }}-${{ steps.rust-cache-key.outputs.target }}";
const draftCompilerCachePrimary = `${draftCompilerCachePrefix}-${cacheLock}`;
const draftCompilerSaveCondition =
  "success() && steps.compiler-cache-restore.outputs.cache-hit != 'true' && steps.compiler-cache-restore.outputs.cache-primary-key != ''";
const draftCompilerSaveKey = "${{ steps.compiler-cache-restore.outputs.cache-primary-key }}";
const cacheSaveKey = "${{ steps.cargo-cache-restore.outputs.cache-primary-key }}";
const draftWorkflowPaths = [
  "Cargo.lock",
  "Cargo.toml",
  "crates/**",
  ".github/scripts/check-runtime-config-boundary.mjs",
  ".github/scripts/check-runtime-config-boundary.test.mjs",
  ".github/scripts/install-linux-vulkan-build-deps.sh",
  ".github/scripts/check-workflow-policy.mjs",
  ".github/scripts/route-ci-proof.mjs",
  ".github/workflows/rust-ci.yml",
  ".github/workflows/source-proof.yml",
  "plugins/codestory/generated-mcp-catalog.json",
  "plugins/codestory/skills/codestory-grounding/**",
  "scripts/generate-codestory-skill-syntax.mjs",
];
const retrievalProducerTriggerPaths = [
  "crates/**/Cargo.toml",
  "vendor/**/Cargo.toml",
  ".github/scripts/install-windows-vulkan-sdk.ps1",
  ".github/workflows/rust-ci.yml",
];
const windowsVulkanInstaller = ".github/scripts/install-windows-vulkan-sdk.ps1";
const windowsNativeGenerator = "Ninja";
const windowsReadyCommand = "cargo test --locked -p codestory-cli --test ready_command";
const windowsReadyProofTopologyDigest = createHash("sha256")
  .update(`${windowsReadyCommand}\nCMAKE_GENERATOR=${windowsNativeGenerator}`)
  .digest("hex");
const windowsReadyProofTopology = `ready-command-v2-${windowsReadyProofTopologyDigest}`;
const windowsInstallerHash = `\${{ hashFiles('${windowsVulkanInstaller}') }}`;
const windowsCachePrimary = [
  cacheRunner,
  "cargo-stable",
  cacheRustVersion,
  cacheTarget,
  "windows",
  windowsReadyProofTopology,
  "generator",
  windowsNativeGenerator.toLowerCase(),
  "cmake",
  "${{ steps.rust-cache-key.outputs.cmake }}",
  "ninja",
  "${{ steps.rust-cache-key.outputs.ninja }}",
  "default-features",
  cacheManifests,
  windowsInstallerHash,
  cacheLock,
].join("-");
const windowsStepSequence = [
  { uses: "actions/checkout@v5", keys: ["uses"] },
  { name: "Install Rust stable", keys: ["name", "shell", "run"] },
  {
    name: "Install checksum-pinned Windows Vulkan SDK",
    keys: ["name", "shell", "run"],
  },
  { name: "Capture Rust cache identity", keys: ["name", "id", "shell", "run"] },
  {
    name: "Restore Windows Cargo inputs and output",
    keys: ["name", "id", "uses", "continue-on-error", "with"],
  },
  {
    name: "Prove Windows ready_command manifest-missing contract",
    keys: ["name", "shell", "run"],
  },
  {
    name: "Save Windows Cargo inputs and output",
    keys: ["name", "if", "uses", "continue-on-error", "with"],
  },
];
const windowsRunCommands = new Map([
  ["Install Rust stable", [
    "rustup toolchain install stable --profile minimal",
    "rustup default stable",
  ]],
  ["Install checksum-pinned Windows Vulkan SDK", [windowsVulkanInstaller]],
  ["Capture Rust cache identity", [
    "$release = rustc -Vv | Select-String '^release: ' | ForEach-Object { $_.ToString().Substring(9) }",
    "$target = rustc -Vv | Select-String '^host: ' | ForEach-Object { $_.ToString().Substring(6) }",
    "$cmake = (cmake --version | Select-Object -First 1) -replace '^cmake version ', ''",
    "$ninja = (ninja --version).Trim()",
    `"version=$release" | Out-File -FilePath $env:GITHUB_OUTPUT -Append`,
    `"target=$target" | Out-File -FilePath $env:GITHUB_OUTPUT -Append`,
    `"cmake=$cmake" | Out-File -FilePath $env:GITHUB_OUTPUT -Append`,
    `"ninja=$ninja" | Out-File -FilePath $env:GITHUB_OUTPUT -Append`,
  ]],
  ["Prove Windows ready_command manifest-missing contract", [windowsReadyCommand]],
]);
const draftStepSequence = [
  { uses: "actions/checkout@v5", keys: ["uses"] },
  { name: "Install Rust stable", keys: ["name", "run"] },
  { name: "Install Linux Vulkan build dependencies", keys: ["name", "run"] },
  { name: "Install pinned sccache", keys: ["name", "uses", "with"] },
  { name: "Configure bounded compiler cache", keys: ["name", "shell", "run"] },
  { name: "Capture Rust cache identity", keys: ["name", "id", "shell", "run"] },
  {
    name: "Restore Cargo inputs and output",
    keys: ["name", "id", "uses", "continue-on-error", "with"],
  },
  {
    name: "Restore compiler objects",
    keys: ["name", "id", "uses", "continue-on-error", "with"],
  },
  { name: "Check formatting", keys: ["name", "run"] },
  { name: "Check immutable runtime configuration boundary", keys: ["name", "run"] },
  { name: "Check the workspace", keys: ["name", "run"] },
  { name: "Check generated CodeStory syntax and MCP catalog", keys: ["name", "run"] },
  { name: "Lint workspace libraries", keys: ["name", "run"] },
  { name: "Prove focused publication contracts", keys: ["name", "run"] },
  {
    name: "Save Cargo inputs and output",
    keys: ["name", "if", "uses", "continue-on-error", "with"],
  },
  {
    name: "Save compiler objects",
    keys: ["name", "if", "uses", "continue-on-error", "with"],
  },
];
const draftRunCommands = new Map([
  ["Configure bounded compiler cache", [
    "{",
    'echo "SCCACHE_DIR=$RUNNER_TEMP/codestory-draft-sccache"',
    'echo "SCCACHE_CACHE_SIZE=1G"',
    'echo "RUSTC_WRAPPER=sccache"',
    'echo "CARGO_INCREMENTAL=0"',
    'echo "CMAKE_C_COMPILER_LAUNCHER=sccache"',
    'echo "CMAKE_CXX_COMPILER_LAUNCHER=sccache"',
    '} >> "$GITHUB_ENV"',
  ]],
  ["Install Rust stable", [
    "rustup toolchain install stable --profile minimal --component clippy --component rustfmt",
    "rustup default stable",
  ]],
  ["Install Linux Vulkan build dependencies", [
    "bash .github/scripts/install-linux-vulkan-build-deps.sh",
  ]],
  ["Capture Rust cache identity", [
    `echo "version=$(rustc -Vv | sed -n 's/^release: //p')" >> "$GITHUB_OUTPUT"`,
    `echo "target=$(rustc -Vv | sed -n 's/^host: //p')" >> "$GITHUB_OUTPUT"`,
  ]],
  ["Check formatting", ["cargo fmt --check"]],
  ["Check immutable runtime configuration boundary", [
    "node --test .github/scripts/check-runtime-config-boundary.test.mjs",
    "node .github/scripts/check-runtime-config-boundary.mjs",
  ]],
  ["Check the workspace", ["cargo check --workspace --locked"]],
  ["Check generated CodeStory syntax and MCP catalog", [
    "cargo build --locked -p codestory-cli",
    "node scripts/generate-codestory-skill-syntax.mjs --check",
  ]],
  ["Lint workspace libraries", [
    "cargo clippy --workspace --lib --locked -- -D warnings",
  ]],
  ["Prove focused publication contracts", draftProofCommands],
]);

function nonCommentLines(value) {
  return String(value ?? "")
    .split(/\r?\n/u)
    .map(line => line.trim())
    .filter(line => line.length > 0 && !line.startsWith("#"));
}

function sameStrings(actual, expected) {
  return JSON.stringify(actual) === JSON.stringify(expected);
}

function hasExactKeys(value, expected) {
  return sameMembers(Object.keys(object(value)), expected);
}

export function draftWorkflowPolicyViolations(workflowValue) {
  const violations = [];
  const workflow = object(workflowValue);
  const triggers = object(workflow.on);
  const pullRequest = object(triggers.pull_request);
  const permissions = object(workflow.permissions);
  const concurrency = object(workflow.concurrency);
  const jobs = object(workflow.jobs);

  add(
    violations,
    hasExactKeys(workflow, ["name", "on", "permissions", "concurrency", "jobs"]),
    "draft source workflow must keep its exact top-level policy shape",
  );
  add(
    violations,
    workflow.name === "Draft source checks",
    "draft source workflow name must remain Draft source checks",
  );
  add(
    violations,
    hasExactKeys(triggers, ["pull_request", "workflow_dispatch"]),
    "draft source workflow must use only pull_request and workflow_dispatch",
  );
  add(
    violations,
    hasExactKeys(pullRequest, ["paths"])
      && list(pullRequest.paths).length === draftWorkflowPaths.length
      && sameMembers(pullRequest.paths, draftWorkflowPaths),
    "draft source pull_request trigger must keep the exact path set",
  );
  add(
    violations,
    triggers.workflow_dispatch === null,
    "draft source workflow_dispatch trigger must remain input-free",
  );
  add(
    violations,
    hasExactKeys(permissions, ["contents"]) && permissions.contents === "read",
    "draft source workflow permissions must remain contents: read only",
  );
  add(
    violations,
    hasExactKeys(concurrency, ["group", "cancel-in-progress"])
      && concurrency.group === "rust-ci-${{ github.event.pull_request.number || github.ref }}"
      && concurrency["cancel-in-progress"] === true,
    "draft source workflow concurrency must keep its exact PR/ref cancellation contract",
  );
  add(
    violations,
    hasExactKeys(jobs, ["linux-draft"]),
    "draft source workflow must contain exactly the linux-draft job",
  );
  return violations;
}

export function retrievalProducerTriggerPolicyViolations(workflowValue) {
  const violations = [];
  const workflow = object(workflowValue);
  add(
    violations,
    includesAll(at(workflow, "on", "pull_request", "paths"), retrievalProducerTriggerPaths),
    "retrieval cache producer pull_request paths must cover every manifest and draft consumer change",
  );
  add(
    violations,
    includesAll(at(workflow, "on", "push", "branches"), ["dev/codestory-next"]),
    "retrieval cache producer must run on dev/codestory-next pushes",
  );
  add(
    violations,
    includesAll(at(workflow, "on", "push", "paths"), retrievalProducerTriggerPaths),
    "retrieval cache producer dev push paths must cover every manifest and draft consumer change",
  );
  return violations;
}

export function windowsManifestProofPolicyViolations(workflowValue) {
  const violations = [];
  const workflow = object(workflowValue);
  const triggers = object(workflow.on);
  const job = object(at(workflow, "jobs", "windows-manifest-missing"));
  const steps = list(job.steps).map(object);

  add(
    violations,
    hasExactKeys(workflow.jobs, ["linux-contracts", "windows-manifest-missing"]),
    "Windows manifest proof workflow must contain exactly linux-contracts and windows-manifest-missing jobs",
  );
  add(
    violations,
    workflow.env === undefined,
    "Windows manifest proof workflow must not define top-level env",
  );
  add(
    violations,
    workflow.defaults === undefined,
    "Windows manifest proof workflow must not define top-level defaults",
  );
  for (const event of ["pull_request", "push"]) {
    add(
      violations,
      includesAll(at(triggers, event, "paths"), [windowsVulkanInstaller]),
      `Windows manifest proof ${event} paths must cover the Vulkan installer`,
    );
  }
  add(
    violations,
    triggers.workflow_dispatch === null,
    "Windows manifest proof workflow_dispatch must remain input-free",
  );
  add(
    violations,
    hasExactKeys(job, ["if", "runs-on", "timeout-minutes", "env", "steps"]),
    "Windows manifest proof job must keep its exact required serial shape",
  );
  add(
    violations,
    job.if === "github.event_name == 'workflow_dispatch'",
    "Windows manifest proof must be workflow-dispatch only",
  );
  add(
    violations,
    !scalarStrings(job).some(value => value.includes("labels")),
    "Windows manifest proof must not be label-triggered",
  );
  add(violations, job["runs-on"] === "windows-latest", "Windows manifest proof must use windows-latest");
  add(violations, job["timeout-minutes"] === 30, "Windows manifest proof timeout must remain 30 minutes");
  add(
    violations,
    hasExactKeys(job.env, ["CODESTORY_EMBED_ALLOW_CPU", "CMAKE_GENERATOR"])
      && job.env.CODESTORY_EMBED_ALLOW_CPU === "1"
      && job.env.CMAKE_GENERATOR === windowsNativeGenerator,
    "Windows manifest proof must explicitly permit CPU runtime execution and use the Ninja native generator",
  );
  add(
    violations,
    steps.length === windowsStepSequence.length,
    "Windows manifest proof must keep its exact serialized step count",
  );
  for (const [index, expected] of windowsStepSequence.entries()) {
    const step = steps[index];
    const matches = expected.name === undefined
      ? step?.uses === expected.uses
      : step?.name === expected.name;
    add(
      violations,
      matches,
      `Windows manifest proof step ${index + 1} must remain ${expected.name ?? expected.uses}`,
    );
    add(
      violations,
      hasExactKeys(step, expected.keys),
      `Windows manifest proof step ${index + 1} must keep the exact ${expected.name ?? expected.uses} key shape`,
    );
  }

  for (const [name, commands] of windowsRunCommands) {
    const step = namedStep(job, name);
    add(violations, step !== undefined, `Windows manifest proof must contain one ${name} step`);
    add(
      violations,
      sameStrings(nonCommentLines(step?.run), commands),
      `Windows manifest proof step ${name} must keep its exact required command sequence`,
    );
    add(
      violations,
      step?.shell === "pwsh",
      `Windows manifest proof step ${name} must use pwsh`,
    );
  }

  const identity = namedStep(job, "Capture Rust cache identity");
  add(
    violations,
    identity?.id === "rust-cache-key",
    "Windows manifest proof cache identity must keep its stable output id",
  );

  const restore = namedStep(job, "Restore Windows Cargo inputs and output");
  const restoreWith = object(restore?.with);
  add(
    violations,
    restore?.id === "cargo-cache-restore",
    "Windows manifest proof cache restore must keep its stable step id",
  );
  add(
    violations,
    restore?.uses === "actions/cache/restore@v5",
    "Windows manifest proof cache restore must use actions/cache/restore@v5",
  );
  add(
    violations,
    restore?.["continue-on-error"] === true && restore?.if === undefined,
    "Windows manifest proof cache restore must remain optional without conditional bypasses",
  );
  add(
    violations,
    hasExactKeys(restoreWith, ["path", "key"]),
    "Windows manifest proof cache restore must use an exact primary without fallbacks",
  );
  add(
    violations,
    sameStrings(nonCommentLines(restoreWith.path), draftCachePaths),
    "Windows manifest proof cache restore must use only Cargo registry, git, and default target paths",
  );
  add(
    violations,
    restoreWith.key === windowsCachePrimary,
    "Windows manifest proof cache key must bind OS, Rust, target, proof topology, default features, manifests, installer, and lock identities",
  );

  const proof = namedStep(job, "Prove Windows ready_command manifest-missing contract");
  const install = namedStep(job, "Install checksum-pinned Windows Vulkan SDK");
  const restoreIndex = steps.indexOf(restore);
  const proofIndex = steps.indexOf(proof);
  add(
    violations,
    steps.indexOf(install) < proofIndex && restoreIndex < proofIndex,
    "Windows manifest proof must install the SDK and restore only compatible output before the Cargo proof",
  );

  const save = namedStep(job, "Save Windows Cargo inputs and output");
  const saveWith = object(save?.with);
  add(
    violations,
    save?.uses === "actions/cache/save@v5",
    "Windows manifest proof cache save must use actions/cache/save@v5",
  );
  add(
    violations,
    save?.["continue-on-error"] === true,
    "Windows manifest proof cache save must remain non-blocking",
  );
  add(
    violations,
    save?.if === cacheSaveCondition,
    "Windows manifest proof cache save must require full proof success and skip exact hits",
  );
  add(
    violations,
    hasExactKeys(saveWith, ["path", "key"]),
    "Windows manifest proof cache save inputs must keep their exact shape",
  );
  add(
    violations,
    sameStrings(nonCommentLines(saveWith.path), draftCachePaths),
    "Windows manifest proof cache save must use the exact restore path set",
  );
  add(
    violations,
    saveWith.key === cacheSaveKey,
    "Windows manifest proof cache save must use the exact primary rather than a matched key",
  );
  add(
    violations,
    proofIndex + 1 === steps.indexOf(save),
    "Windows manifest proof cache save must immediately follow the successful Cargo proof",
  );

  return violations;
}

export function draftSourcePolicyViolations(jobValue, retrievalJobValue) {
  const violations = [];
  const job = object(jobValue);
  const retrievalJob = object(retrievalJobValue);
  const steps = list(job.steps).map(object);

  add(
    violations,
    retrievalJob["timeout-minutes"] === 60,
    "retrieval cache producer timeout must remain 60 minutes",
  );
  add(
    violations,
    hasExactKeys(job, ["name", "runs-on", "timeout-minutes", "steps"]),
    "draft source job must keep its exact required serial shape",
  );
  add(
    violations,
    job.name === "Ubuntu draft source checks",
    "draft source job name must remain Ubuntu draft source checks",
  );
  add(violations, job["runs-on"] === "ubuntu-latest", "draft source job must use ubuntu-latest");
  add(violations, job["timeout-minutes"] === 45, "draft source job timeout must remain 45 minutes");
  add(violations, job.env === undefined && job.defaults === undefined, "draft source job must not override the proof environment or defaults");
  add(violations, job["continue-on-error"] === undefined && job.strategy === undefined, "draft source job must remain one required serial lane");
  add(violations, steps.length === draftStepSequence.length, "draft source job must keep its exact serialized step count");
  for (const [index, expected] of draftStepSequence.entries()) {
    const step = steps[index];
    const matches = expected.name === undefined
      ? step?.uses === expected.uses
      : step?.name === expected.name;
    add(violations, matches, `draft source step ${index + 1} must remain ${expected.name ?? expected.uses}`);
    add(
      violations,
      hasExactKeys(step, expected.keys),
      `draft source step ${index + 1} must keep the exact ${expected.name ?? expected.uses} key shape`,
    );
  }

  for (const [name, commands] of draftRunCommands) {
    const step = namedStep(job, name);
    add(violations, step !== undefined, `draft source job must contain one ${name} step`);
    add(violations, sameStrings(nonCommentLines(step?.run), commands), `draft source step ${name} must keep its exact serial command sequence`);
    add(violations, step?.["continue-on-error"] === undefined && step?.if === undefined, `draft source step ${name} must remain required`);
    add(violations, step?.env === undefined && step?.["working-directory"] === undefined, `draft source step ${name} must use the shared default build environment`);
  }

  const identity = namedStep(job, "Capture Rust cache identity");
  add(violations, identity?.id === "rust-cache-key" && identity?.shell === "bash", "draft source cache identity must keep its stable bash output contract");

  const restore = namedStep(job, "Restore Cargo inputs and output");
  const restoreWith = object(restore?.with);
  add(violations, restore?.id === "cargo-cache-restore", "draft source cache restore must keep its stable step id");
  add(violations, restore?.uses === "actions/cache/restore@v5", "draft source cache restore must use actions/cache/restore@v5");
  add(violations, restore?.["continue-on-error"] === true && restore?.if === undefined, "draft source cache restore must remain optional without conditional bypasses");
  add(
    violations,
    hasExactKeys(restoreWith, ["path", "key", "restore-keys"]),
    "draft source cache restore inputs must keep their exact key shape",
  );
  add(violations, sameStrings(nonCommentLines(restoreWith.path), draftCachePaths), "draft source cache restore must use only the Cargo registry, git, and default target paths");
  add(violations, restoreWith.key === draftCachePrimary, "draft source cache primary must bind the v2 platform, toolchain, target, proof topology, feature, manifest, and lock identity");
  add(violations, sameStrings(nonCommentLines(restoreWith["restore-keys"]), draftCacheRestoreKeys), "draft source cache fallbacks must keep the exact seeded retrieval, prior draft, then prior retrieval order and omit only the lock identity from prior prefixes");

  const sccache = namedStep(job, "Install pinned sccache");
  add(violations, sccache?.uses === sccacheAction, "draft source sccache must use the pinned mozilla-actions release");
  add(
    violations,
    object(sccache?.with).version === sccacheVersion
      && object(sccache?.with).disable_annotations === true,
    "draft source sccache must pin the shared sccache version without annotations",
  );

  const compilerRestore = namedStep(job, "Restore compiler objects");
  const compilerRestoreWith = object(compilerRestore?.with);
  add(violations, compilerRestore?.id === "compiler-cache-restore", "draft compiler cache restore must keep its stable step id");
  add(violations, compilerRestore?.uses === "actions/cache/restore@v5", "draft compiler cache restore must use actions/cache/restore@v5");
  add(violations, compilerRestore?.["continue-on-error"] === true && compilerRestore?.if === undefined, "draft compiler cache restore must remain optional without conditional bypasses");
  add(violations, compilerRestoreWith.path === draftCompilerCachePath, "draft compiler cache must live in the runner-temp sccache directory");
  add(violations, compilerRestoreWith.key === draftCompilerCachePrimary, "draft compiler cache primary must bind platform, toolchain, target, and lock identity");
  add(violations, sameStrings(nonCommentLines(compilerRestoreWith["restore-keys"]), [`${draftCompilerCachePrefix}-`]), "draft compiler cache fallback must omit only the lock identity");

  const compilerSave = namedStep(job, "Save compiler objects");
  const compilerSaveWith = object(compilerSave?.with);
  add(violations, compilerSave?.uses === "actions/cache/save@v5", "draft compiler cache save must use actions/cache/save@v5");
  add(violations, compilerSave?.["continue-on-error"] === true, "draft compiler cache save must remain non-blocking");
  add(violations, compilerSave?.if === draftCompilerSaveCondition, "draft compiler cache must save only on a successful run that missed its primary key");
  add(violations, compilerSaveWith.path === draftCompilerCachePath && compilerSaveWith.key === draftCompilerSaveKey, "draft compiler cache save must publish the restored primary key path");

  const retrievalRestore = namedStep(retrievalJob, "Restore Cargo registry, git sources, and build output");
  const retrievalRestoreWith = object(retrievalRestore?.with);
  add(
    violations,
    hasExactKeys(retrievalRestore, ["name", "id", "uses", "continue-on-error", "with"]),
    "retrieval cache producer restore must keep its exact step shape",
  );
  add(violations, retrievalRestore?.id === "cargo-cache-restore", "retrieval cache producer must keep its stable restore id");
  add(violations, retrievalRestore?.uses === "actions/cache/restore@v5", "retrieval cache producer must use actions/cache/restore@v5");
  add(violations, retrievalRestore?.["continue-on-error"] === true && retrievalRestore?.if === undefined, "retrieval cache producer restore must remain non-blocking without conditional bypasses");
  add(
    violations,
    hasExactKeys(retrievalRestoreWith, ["path", "key"]),
    "retrieval cache producer restore inputs must keep their exact key shape",
  );
  add(violations, sameStrings(nonCommentLines(retrievalRestoreWith.path), draftCachePaths), "retrieval cache producer must retain the proof-compatible path set");
  add(violations, retrievalRestoreWith.key === retrievalCachePrimary, "retrieval cache producer key must match the draft exact-lock, manifest, feature, and proof-topology fallback");

  const retrievalSeed = namedStep(retrievalJob, "Seed draft proof test-profile artifacts");
  add(
    violations,
    hasExactKeys(retrievalSeed, ["name", "run"]),
    "retrieval cache producer seed must keep its exact required step shape",
  );
  add(
    violations,
    sameStrings(nonCommentLines(retrievalSeed?.run), draftSeedCommands),
    "retrieval cache producer must seed the exact five test-profile targets in serial order",
  );

  const retrievalSave = namedStep(retrievalJob, "Save Cargo registry, git sources, and build output");
  const retrievalSaveWith = object(retrievalSave?.with);
  add(
    violations,
    hasExactKeys(retrievalSave, ["name", "if", "uses", "continue-on-error", "with"]),
    "retrieval cache producer save must keep its exact post-proof step shape",
  );
  add(violations, retrievalSave?.uses === "actions/cache/save@v5", "retrieval cache producer must use actions/cache/save@v5");
  add(violations, retrievalSave?.["continue-on-error"] === true, "retrieval cache producer save must remain non-blocking");
  add(violations, retrievalSave?.if === cacheSaveCondition, "retrieval cache producer must save only after every retrieval and seed proof succeeds");
  add(
    violations,
    hasExactKeys(retrievalSaveWith, ["path", "key"]),
    "retrieval cache producer save inputs must keep their exact key shape",
  );
  add(violations, sameStrings(nonCommentLines(retrievalSaveWith.path), draftCachePaths), "retrieval cache producer save must retain the proof-compatible path set");
  add(violations, retrievalSaveWith.key === cacheSaveKey, "retrieval cache producer must save its exact primary rather than a matched key");
  const retrievalSteps = list(retrievalJob.steps).map(object);
  add(
    violations,
    retrievalSteps.indexOf(retrievalSeed) + 1 === retrievalSteps.indexOf(retrievalSave),
    "retrieval cache producer must seed the exact proof targets immediately before saving",
  );

  const save = namedStep(job, "Save Cargo inputs and output");
  const saveWith = object(save?.with);
  add(violations, save?.uses === "actions/cache/save@v5", "draft source cache promotion must use actions/cache/save@v5");
  add(violations, save?.["continue-on-error"] === true, "draft source cache promotion must remain non-blocking");
  add(
    violations,
    hasExactKeys(saveWith, ["path", "key"]),
    "draft source cache promotion inputs must keep their exact key shape",
  );
  add(
    violations,
    save?.if === cacheSaveCondition,
    "draft source cache promotion must require complete proof and a partial or missing primary",
  );
  add(violations, sameStrings(nonCommentLines(saveWith.path), draftCachePaths), "draft source cache promotion must use the exact restore path set");
  add(violations, saveWith.key === cacheSaveKey, "draft source cache promotion must save the exact primary rather than a matched fallback");

  return violations;
}

export const releaseEvidenceWorkflowRef = "./.github/workflows/release-candidate-evidence.yml";

export function macosCliDistributionViolations(assessmentStep, executionStep, quarantinedPath) {
  const violations = [];
  const assessment = executableRunText(String(assessmentStep?.run ?? ""));
  const execution = executableRunText(String(executionStep?.run ?? ""));
  const assessmentLines = assessment.split("\n");
  const executionLines = execution.split("\n");
  const lineHas = (lines, ...fragments) => lines.some(line => fragments.every(fragment => line.includes(fragment)));
  add(violations, lineHas(assessmentLines, "xattr -w com.apple.quarantine", quarantinedPath), "macOS CLI proof must quarantine the assessed executable");
  add(violations, lineHas(assessmentLines, "xattr -p com.apple.quarantine", quarantinedPath), "macOS CLI proof must record the executable quarantine");
  add(violations, lineHas(assessmentLines, "spctl --assess --type execute --verbose=4", quarantinedPath), "macOS CLI proof must retain the spctl diagnostic for that executable");
  add(violations, assessment.includes("spctl_status=$?"), "macOS CLI proof must record the spctl diagnostic status");
  add(violations, assessment.includes("does not seem to be an app"), "macOS CLI proof must recognize the bare-executable spctl result");
  add(violations, !/(^|\n)\s*accepted=false\s*($|\n)/u.test(assessment), "macOS CLI proof must not require spctl application acceptance");
  add(violations, lineHas(executionLines, quarantinedPath, "--version") && lineHas(executionLines, quarantinedPath, "--help"), "macOS CLI proof must execute that quarantined binary's version and help");
  return violations;
}

export function releaseEvidenceApprovalViolations(callerJobs, calledWorkflow) {
  const violations = [];
  const file = releaseEvidenceWorkflowRef.slice(releaseEvidenceWorkflowRef.lastIndexOf("/") + 1);
  for (const [callerFile, callerJob, passesApproval] of callerJobs) {
    const job = object(callerJob);
    add(
      violations,
      callerJob !== undefined,
      `${callerFile} must contain job release-evidence`,
    );
    add(
      violations,
      job.uses === releaseEvidenceWorkflowRef,
      `${callerFile} release-evidence must call the evidence workflow`,
    );
    add(
      violations,
      object(job.with).source_run_id === "${{ inputs.source_run_id }}",
      `${callerFile} release-evidence must forward source_run_id`,
    );
    const secrets = object(job.secrets);
    const secret = secrets.CODESTORY_RELEASE_EVIDENCE_APPROVAL_JSON;
    add(
      violations,
      passesApproval
        ? secret === "${{ secrets.CODESTORY_RELEASE_EVIDENCE_APPROVAL_JSON }}"
          && Object.keys(secrets).length === 1
        : job.secrets === undefined,
      passesApproval
        ? `${callerFile} release-evidence must pass only the named approval secret`
        : `${callerFile} release-evidence must not receive caller secrets`,
    );
  }
  add(
    violations,
    object(at(
      calledWorkflow, "on", "workflow_call", "secrets",
      "CODESTORY_RELEASE_EVIDENCE_APPROVAL_JSON",
    )).required === false,
    `${file} approval must be an optional caller secret`,
  );

  const job = object(at(calledWorkflow, "jobs", "measure"));
  add(
    violations,
    job.environment === "release-evidence",
    `${file} approval must remain gated by the release-evidence environment`,
  );
  const evaluation = namedStep(job, "Produce and evaluate same-SHA candidate");
  add(
    violations,
    object(evaluation?.env).APPROVAL_JSON
      === "${{ secrets.CODESTORY_RELEASE_EVIDENCE_APPROVAL_JSON }}",
    `${file} approval must use the explicitly passed release secret`,
  );
  requireStepRun(violations, file, job, "Produce and evaluate same-SHA candidate", [
    'if [ -n "$SOURCE_RUN_ID" ] && [ -z "$APPROVAL_JSON" ]; then',
    "Protected release-evidence approval is required for source-run re-evaluation.",
    "exit 1",
  ]);
  return violations;
}

export function parseWorkflow(source, file = "workflow") {
  const document = parseDocument(source, {
    lineCounter: new LineCounter(),
    prettyErrors: true,
    schema: "core",
    strict: true,
    uniqueKeys: true,
  });
  if (document.errors.length > 0) {
    throw new Error(document.errors.map(error => error.message).join("\n"));
  }
  const parsed = document.toJS({ maxAliasCount: 50 });
  if (parsed === null || typeof parsed !== "object" || Array.isArray(parsed)) {
    throw new Error(`${file} must contain one YAML mapping`);
  }
  return parsed;
}

export function loadWorkflows(root = workflowRoot) {
  const loaded = new Map();
  for (const file of fs.readdirSync(root).filter(name => /\.ya?ml$/u.test(name)).sort()) {
    const source = fs.readFileSync(path.join(root, file), "utf8");
    loaded.set(file, parseWorkflow(source, file));
  }
  return loaded;
}

function trigger(workflow, name) {
  return object(workflow.on)[name];
}

function concurrencyCancels(workflow) {
  return object(workflow.concurrency)["cancel-in-progress"] === true;
}

function executableCargoLines(run) {
  return run
    .split(/\r?\n/u)
    .map((line, index) => ({ line, number: index + 1 }))
    .filter(({ line }) =>
      /^\s*(?:[A-Z_][A-Z0-9_]*=\S+\s+)*(?:sudo\s+)?cargo\s+(?:build|check|test|clippy|doc|run)\b/u.test(line),
    );
}

function walk(value, visit, trail = []) {
  if (Array.isArray(value)) {
    value.forEach((item, index) => walk(item, visit, [...trail, index]));
    return;
  }
  if (value === null || typeof value !== "object") return;
  for (const [key, child] of Object.entries(value)) {
    visit(key, child, [...trail, key]);
    walk(child, visit, [...trail, key]);
  }
}

export function basicWorkflowViolations(file, workflow) {
  const violations = [];
  const triggers = object(workflow.on);
  if (triggers.pull_request !== undefined || triggers.pull_request_target !== undefined) {
    add(
      violations,
      concurrencyCancels(workflow),
      `${file} pull-request runs must cancel stale work`,
    );
  }

  // Least privilege is not the repository default, so every workflow states its own scopes.
  add(
    violations,
    workflow.permissions !== undefined,
    `${file} must declare a top-level permissions block`,
  );

  // Reusable-workflow callers inherit the callee's budget and cannot set one themselves, so the
  // rule applies only to jobs that own steps.
  for (const [jobName, rawJob] of Object.entries(object(workflow.jobs))) {
    const job = object(rawJob);
    if (!Array.isArray(job.steps)) continue;
    add(
      violations,
      job["timeout-minutes"] !== undefined,
      `${file} jobs.${jobName} must declare timeout-minutes`,
    );
  }

  walk(workflow, (key, value, trail) => {
    if (key === "key" && typeof value === "string" && value.includes("github.sha")) {
      violations.push(`${file} ${trail.join(".")} Cargo cache key must not include commit SHA`);
    }
    if (key !== "uses" || typeof value !== "string" || value.startsWith("./")) return;
    const separator = value.lastIndexOf("@");
    if (separator < 0) {
      violations.push(`${file} ${value} is missing an action ref`);
      return;
    }
    const owner = value.slice(0, separator).split("/")[0];
    const ref = value.slice(separator + 1);
    if (!trustedActionOwners.has(owner) && !fullSha.test(ref)) {
      violations.push(`${file} ${value} must pin third-party actions to a full-length SHA`);
    }
  });

  for (const [jobName, job] of Object.entries(object(workflow.jobs))) {
    for (const [stepIndex, step] of list(object(job).steps).entries()) {
      if (typeof step?.run !== "string") continue;
      for (const { line, number } of executableCargoLines(step.run)) {
        if (!/(?:^|\s)--locked(?:\s|$)/u.test(line)) {
          violations.push(
            `${file} jobs.${jobName}.steps.${stepIndex}.run:${number} dependency-resolving Cargo command must use --locked`,
          );
        }
      }
    }
  }
  return violations;
}

export function packagedPrSigningViolations(workflow) {
  const violations = [];
  const job = object(object(workflow.jobs)["packaged-proof"]);
  add(
    violations,
    object(job.with).sign_macos === false,
    "packaged-platform-pr.yml packaged-proof must set sign_macos to false",
  );
  add(
    violations,
    job.secrets === undefined,
    "packaged-platform-pr.yml packaged-proof must not receive caller secrets",
  );
  const scalars = scalarStrings(workflow);
  let referencesAppleSecret = false;
  walk(workflow, (key, value) => {
    if (/^APPLE_[A-Z0-9_]+$/u.test(key)) referencesAppleSecret = true;
    if (typeof value === "string" && /\bAPPLE_[A-Z0-9_]+\b/u.test(value)) {
      referencesAppleSecret = true;
    }
  });
  add(
    violations,
    !referencesAppleSecret,
    "packaged-platform-pr.yml must not reference Apple secret identifiers",
  );
  add(
    violations,
    !scalars.some(value => value.includes("macos-release-signing")),
    "packaged-platform-pr.yml must not reference the release signing environment",
  );
  return violations;
}

export function notaryStepViolations(step) {
  const run = typeof step?.run === "string" ? step.run : "";
  return run
    .split(/\r?\n/u)
    .some(line => /^\s*--wait(?:\s|\\|$)/u.test(line) || /notarytool\s+submit.*\s--wait(?:\s|$)/u.test(line))
    ? ["notarization must poll explicitly instead of using notarytool --wait"]
    : [];
}

function validateLockedSetupSurfaces(violations) {
  const contracts = new Map([
    [
      path.join(".cargo", "config.toml"),
      [
        'retrieval-setup = "run --locked -p codestory-cli',
        'retrieval-status = "run --locked -p codestory-cli',
      ],
    ],
    [
      path.join("scripts", "codex-worktree-setup.mjs"),
      [
        '["build", "--release", "--locked", "-p", "codestory-cli"]',
        "prepare-embedded-model.mjs",
        "CODESTORY_EMBED_MODEL_SOURCE",
      ],
    ],
    [
      path.join("plugins", "codestory", "skills", "codestory-grounding", "scripts", "setup.sh"),
      [
        "cargo build --release --locked -p codestory-cli",
        "prepare-embedded-model.mjs",
        "CODESTORY_EMBED_MODEL_SOURCE",
      ],
    ],
    [
      path.join("plugins", "codestory", "skills", "codestory-grounding", "scripts", "setup.ps1"),
      [
        '@("build", "--release", "--locked", "-p", "codestory-cli"',
        "prepare-embedded-model.mjs",
        "CODESTORY_EMBED_MODEL_SOURCE",
      ],
    ],
  ]);
  for (const [file, fragments] of contracts) {
    const source = fs.readFileSync(file, "utf8");
    for (const fragment of fragments) {
      add(violations, source.includes(fragment), `${file} must preserve locked Cargo contract ${fragment}`);
    }
  }
}

function validateIssueWorkflows(workflows, violations) {
  const sagaFile = "saga-issue-link-guard.yml";
  const saga = workflows.get(sagaFile);
  if (!saga) {
    violations.push(`${sagaFile} must exist`);
  } else {
    add(violations, trigger(saga, "pull_request_target") !== undefined, `${sagaFile} must use pull_request_target`);
    add(violations, object(saga.permissions)["pull-requests"] === "read", `${sagaFile} must read pull requests`);
    const job = requireJob(violations, sagaFile, saga, "require-closing-issue-link");
    for (const fragment of ["codex/", "review/codestory-saga-", "[codex]", "saga:codestory-intelligence"]) {
      add(violations, String(job.if ?? "").includes(fragment), `${sagaFile} guarded condition must include ${fragment}`);
    }
    requireStepRun(violations, sagaFile, job, "Check PR issue relationship", [
      "close[sd]?|fix(?:e[sd])?|resolve[sd]?",
      "#\\d+|https://github\\.com/TheGreenCedar/CodeStory/issues/\\d+",
    ]);
  }

  const closeFile = "close-dev-issues.yml";
  const close = workflows.get(closeFile);
  if (!close) {
    violations.push(`${closeFile} must exist`);
  } else {
    add(violations, includesAll(at(close, "on", "push", "branches"), ["dev/codestory-next"]), `${closeFile} must run on dev/codestory-next pushes`);
    add(violations, object(close.permissions).issues === "write", `${closeFile} must write issues`);
    add(violations, object(close.permissions)["pull-requests"] === "read", `${closeFile} must read pull requests`);
    const job = requireJob(violations, closeFile, close, "close-linked-issues");
    requireStepRun(violations, closeFile, job, "Close issues referenced by the merged PR", [
      'commit = event["after"]',
      'pull_request.get("merged_at")',
      'pull_request.get("merge_commit_sha") == commit',
      'if "pull_request" in issue:',
      '"state_reason=completed"',
      "https://github\\.com/TheGreenCedar/CodeStory/issues/(\\d+)",
    ]);
  }
}

function validatePluginAndDraftWorkflows(workflows, violations, graph) {
  const pluginFile = "plugin-static.yml";
  const plugin = workflows.get(pluginFile);
  if (!plugin) {
    violations.push(`${pluginFile} must exist`);
  } else {
    const requiredPaths = [
      "plugins/codestory/**",
      ".github/scripts/check-workflow-policy.mjs",
      ".github/scripts/check-workflow-policy.test.mjs",
      ".github/scripts/cargo-cache-contract.mjs",
      ".github/scripts/cargo-cache-contract.test.mjs",
      ".github/scripts/install-codestory-marketplace-proof.mjs",
      ".github/scripts/install-codestory-marketplace-proof.test.mjs",
      ".github/scripts/fixtures/workflow-policy-invalid.json",
      ".github/scripts/fixtures/actionlint-invalid.yml",
      ".github/scripts/run-actionlint.mjs",
      ".github/scripts/run-actionlint.test.mjs",
      ".github/actionlint.yaml",
      "release-claims.json",
      "scripts/codestory-release-claims.mjs",
      "scripts/codestory-release-closeout.mjs",
      "scripts/codestory-release-evidence-gate.mjs",
      "scripts/tests/codestory-release-claims.test.mjs",
      "scripts/tests/codestory-release-closeout.test.mjs",
      "scripts/tests/codestory-release-evidence-gate.test.mjs",
      "scripts/tests/fixtures/release-claims/**",
      "benchmarks/release-evidence/**",
      ".github/workflows/**",
      ".github/workflows/release.yml",
      ".github/workflows/packaged-platform-pr.yml",
      ".github/workflows/packaged-platform-proof.yml",
      ".github/workflows/macos-metal-proof.yml",
      ".github/workflows/windows-vulkan-proof.yml",
      ".github/workflows/retrieval-engine-smoke.yml",
      ".github/workflows/source-proof.yml",
      ".github/workflows/repo-scale-stats.yml",
      "package.json",
      "package-lock.json",
      "scripts/codex-worktree-setup.*",
      "scripts/install-codestory.ps1",
      "scripts/prepare-embedded-model.mjs",
      "scripts/tests/prepare-embedded-model.test.mjs",
      "crates/codestory-llama-sys/model-contract.json",
      "crates/codestory-llama-sys/build.rs",
      "crates/codestory-llama-sys/model_staging.rs",
      "crates/codestory-llama-sys/Cargo.toml",
      "crates/codestory-llama-sys/tests/model_staging.rs",
      "scripts/release-evidence/**",
      "scripts/release-evidence/serde-json-codestory-project.json",
      "scripts/tests/release-evidence-runner-contract.test.mjs",
    ];
    for (const event of ["pull_request", "push"]) {
      add(violations, includesAll(at(plugin, "on", event, "paths"), requiredPaths), `${pluginFile} ${event} paths must cover policy and release surfaces`);
    }
    add(violations, includesAll(at(plugin, "on", "push", "branches"), ["dev/codestory-next"]), `${pluginFile} must run on dev pushes`);
    const job = requireJob(violations, pluginFile, plugin, "plugin-static");
    requireStepRun(violations, pluginFile, job, "Install workflow policy dependencies", ["npm ci --ignore-scripts"]);
    requireStepRun(violations, pluginFile, job, "Check workflow policy", [
      "node .github/scripts/check-workflow-policy.mjs",
      "node --test .github/scripts/check-workflow-policy.test.mjs",
      "node --test .github/scripts/cargo-cache-contract.test.mjs",
    ]);
    requireStepRun(violations, pluginFile, job, "Check plugin static wiring", ["node --test plugins/codestory/tests/plugin-static.test.mjs"]);
    requireStepRun(violations, pluginFile, job, "Check embedded model preparation", ["node --test scripts/tests/prepare-embedded-model.test.mjs"]);
    requireStepRun(violations, pluginFile, job, "Check release claim and evidence contracts", [
      "scripts/tests/release-evidence-runner-contract.test.mjs",
    ]);
    requireStepRun(violations, pluginFile, job, "Check workflow syntax", [
      "node --test .github/scripts/run-actionlint.test.mjs",
      "node .github/scripts/run-actionlint.mjs",
    ]);
    requireStepRun(violations, pluginFile, job, "Check release claim and evidence contracts", [
      "scripts/tests/codestory-release-claims.test.mjs",
      "scripts/tests/codestory-release-closeout.test.mjs",
      "scripts/tests/codestory-release-evidence-gate.test.mjs",
    ]);
    requireStepRun(violations, pluginFile, job, "Check CI proof routing fixtures", ["node .github/scripts/route-ci-proof.mjs --self-test"]);
    requireStepRun(violations, pluginFile, job, "Check packaged proof harness", ["python .github/scripts/check-packaged-agent-proof.py --self-test"]);
    requireStepRun(violations, pluginFile, job, "Check real Codex marketplace installation", [
      "node --test .github/scripts/install-codestory-marketplace-proof.test.mjs",
    ]);
  }

  const rustFile = "rust-ci.yml";
  const rust = workflows.get(rustFile);
  if (!rust) {
    violations.push(`${rustFile} must exist`);
  } else {
    for (const violation of draftWorkflowPolicyViolations(rust)) {
      violations.push(`${rustFile} ${violation}`);
    }
    add(violations, trigger(rust, "push") === undefined, `${rustFile} draft checks must not run on push`);
    add(violations, includesAll(at(rust, "on", "pull_request", "paths"), [
      "Cargo.lock",
      "Cargo.toml",
      "crates/**",
      "plugins/codestory/generated-mcp-catalog.json",
      "plugins/codestory/skills/codestory-grounding/**",
      "scripts/generate-codestory-skill-syntax.mjs",
    ]), `${rustFile} must cover workspace source and generated catalog changes`);
    const job = requireJob(violations, rustFile, rust, "linux-draft");
    const retrievalWorkflow = workflows.get(retrievalFile);
    for (const violation of retrievalProducerTriggerPolicyViolations(retrievalWorkflow)) {
      violations.push(`${retrievalFile} ${violation}`);
    }
    const retrievalJob = object(at(
      retrievalWorkflow,
      "jobs",
      "linux-contracts",
    ));
    for (const violation of draftSourcePolicyViolations(job, retrievalJob)) {
      violations.push(`${rustFile} ${violation}`);
    }
  }

  const sourceFile = "source-proof.yml";
  const source = workflows.get(sourceFile);
  if (!source) {
    violations.push(`${sourceFile} must exist`);
  } else {
    const promotion = graph.workflow_policy.promotion;
    const sourceConcurrency = [
      "source-proof-",
      promotion.proof_run_sha_expression,
      "-${{ inputs.proof_key || inputs.pr_number || github.event.pull_request.number || github.ref }}-",
      "${{ github.event.action == 'labeled' && github.event.label.name || 'dispatch' }}",
    ].join("");
    add(
      violations,
      sameMembers(at(source, "on", "pull_request", "types"), promotion.required_events),
      `${sourceFile} pull request trigger must be label-only`,
    );
    add(
      violations,
      at(source, "concurrency", "group") === sourceConcurrency,
      `${sourceFile} concurrency must bind the Actions SHA, proof identity, and exact label`,
    );
    add(
      violations,
      String(at(source, "on", "workflow_dispatch", "inputs", "pr_number", "description") ?? "")
        .includes(promotion.manual_pr_ref_hint),
      `${sourceFile} manual PR input must require ${promotion.manual_pr_ref_hint}`,
    );
    add(violations, trigger(source, "pull_request_target") === undefined, `${sourceFile} must not execute pull-request code through pull_request_target`);
    const resolve = requireJob(violations, sourceFile, source, "resolve");
    add(
      violations,
      resolve.if === "github.event_name != 'pull_request' || (github.event.action == 'labeled' && github.event.label.name == 'review-accepted')",
      `${sourceFile} resolve job must execute dispatch/call runs and only review-accepted labeled PR runs`,
    );
    requireStepRun(violations, sourceFile, resolve, "Resolve trusted exact head", [
      'test "$EVENT_HEAD_REPO" = "$GITHUB_REPOSITORY"',
      'test "$current_head" = "$EVENT_HEAD_SHA"',
      'head_ref="$(jq -r \'.head.ref\'',
      'test "$GITHUB_REF" = "refs/heads/$head_ref"',
      'test "$GITHUB_SHA" = "$EXPECTED_HEAD_SHA"',
      'test "$GITHUB_SHA" = "$CALLER_REF"',
      "--ref $head_ref",
    ]);
    requireExactResolverContract(violations, sourceFile, resolve, sourceResolverContractDigest);
    const full = requireJob(violations, sourceFile, source, "full-source-gate");
    add(violations, sameMembers(needs(full), ["resolve"]), `${sourceFile} full source gate must need resolve`);
    add(
      violations,
      object(source.env).SCCACHE_VERSION === sccacheVersion
        && object(source.env).SCCACHE_CACHE_SIZE === sccacheCacheSize
        && object(source.env).CARGO_DEPENDENCY_CACHE_MAX_BYTES === "1073741824",
      `${sourceFile} must pin bounded compiler and dependency caches`,
    );
    const sccacheSetup = namedStep(full, "Install pinned sccache");
    add(
      violations,
      sccacheSetup?.uses === sccacheAction
        && object(sccacheSetup?.with).version === "${{ env.SCCACHE_VERSION }}",
      `${sourceFile} must install the pinned sccache action and binary`,
    );
    requireStepRun(violations, sourceFile, full, "Configure bounded compiler cache", [
      "CARGO_HOME=$RUNNER_TEMP/codestory-source-cargo",
      "SCCACHE_DIR=$RUNNER_TEMP/codestory-source-sccache",
      "SCCACHE_CACHE_SIZE=$SCCACHE_CACHE_SIZE",
      "RUSTC_WRAPPER=sccache",
      "CARGO_INCREMENTAL=0",
      "CMAKE_C_COMPILER_LAUNCHER=sccache",
      "CMAKE_CXX_COMPILER_LAUNCHER=sccache",
    ]);
    const identity = namedStep(full, "Capture reusable build cache contract");
    const identityRun = executableRunText(String(identity?.run ?? ""));
    add(
      violations,
      identity?.id === "build-cache"
        && identity?.shell === "bash"
        && identityRun.includes(`--namespace ${promotion.source_cache_namespace}`)
        && identityRun.includes('--exact-sha "$EXACT_SHA"')
        && identityRun.includes('--os "$RUNNER_OS"')
        && identityRun.includes('--target "$target"')
        && identityRun.includes('--rust-version "$rust_version"')
        && identityRun.includes("--features workspace-test-default-and-clippy-all-targets-all-features")
        && identityRun.includes('--native-toolchain "$native_toolchain"')
        && identityRun.includes("--generator unix-makefiles")
        && identityRun.includes('--cmake-version "$cmake_version"')
        && identityRun.includes('--ninja-version "$ninja_version"')
        && identityRun.includes("--lock-file Cargo.lock")
        && identityRun.includes("--cargo-config .cargo/config.toml")
        && identityRun.includes("--sccache-version \"$SCCACHE_VERSION\"")
        && identityRun.includes(".cargo/llama-dynamic-backends.cmake")
        && identityRun.includes("git ls-files '*Cargo.toml'")
        && identityRun.includes("model-contract.json")
        && identityRun.includes("--identity cargo_incremental=0"),
      `${sourceFile} must compute one reusable compiler compatibility contract`,
    );
    const dependencyRestore = namedStep(full, "Restore Cargo dependency inputs");
    const dependencyRestoreWith = object(dependencyRestore?.with);
    add(
      violations,
      dependencyRestore?.uses === "actions/cache/restore@v5"
        && dependencyRestore?.["continue-on-error"] === true
        && sameMembers(cachePaths(dependencyRestore), [
          "${{ runner.temp }}/codestory-source-cargo/registry",
          "${{ runner.temp }}/codestory-source-cargo/git",
        ])
        && dependencyRestoreWith.key === "${{ steps.build-cache.outputs.dependency-key }}"
        && dependencyRestoreWith["restore-keys"] === undefined,
      `${sourceFile} dependency cache must be exact-input-only and exclude compiler output`,
    );
    const compilerRestore = namedStep(full, "Restore compatible compiler objects");
    const compilerRestoreWith = object(compilerRestore?.with);
    add(
      violations,
      compilerRestore?.uses === "actions/cache/restore@v5"
        && compilerRestore?.["continue-on-error"] === true
        && sameMembers(cachePaths(compilerRestore), ["${{ runner.temp }}/codestory-source-sccache"])
        && compilerRestoreWith.key === "${{ steps.build-cache.outputs.compiler-key }}"
        && String(compilerRestoreWith["restore-keys"] ?? "").trim()
          === "${{ steps.build-cache.outputs.compiler-prefix }}",
      `${sourceFile} compiler cache must restore the newest compatible prior candidate`,
    );
    const dependencySave = namedStep(full, "Save Cargo dependency inputs");
    const compilerSave = namedStep(full, "Save compiler objects after compilation");
    requireStepRun(violations, sourceFile, full, "Bound Cargo dependency cache", [
      "--max-bytes \"$CARGO_DEPENDENCY_CACHE_MAX_BYTES\"",
      "--path \"$CARGO_HOME/registry\"",
      "--path \"$CARGO_HOME/git\"",
    ]);
    add(
      violations,
      dependencySave?.uses === "actions/cache/save@v5"
        && String(dependencySave?.if ?? "").includes("always()")
        && String(dependencySave?.if ?? "").includes("steps.compile-workspace.outcome == 'success'")
        && String(dependencySave?.if ?? "")
          .includes("steps.cargo-dependency-cache-size.outputs.within-limit == 'true'")
        && object(dependencySave?.with).key
          === "${{ steps.cargo-dependency-cache.outputs.cache-primary-key }}"
        && sameMembers(cachePaths(dependencySave), [
          "${{ runner.temp }}/codestory-source-cargo/registry",
          "${{ runner.temp }}/codestory-source-cargo/git",
        ]),
      `${sourceFile} dependency cache must save immediately after successful compilation`,
    );
    add(
      violations,
      compilerSave?.uses === "actions/cache/save@v5"
        && String(compilerSave?.if ?? "").includes("always()")
        && String(compilerSave?.if ?? "").includes("steps.compile-workspace.outcome == 'success'")
        && object(compilerSave?.with).key
          === "${{ steps.compiler-cache-restore.outputs.cache-primary-key }}"
        && sameMembers(cachePaths(compilerSave), ["${{ runner.temp }}/codestory-source-sccache"]),
      `${sourceFile} compiler cache must save a new exact-SHA suffix after successful compilation`,
    );
    add(
      violations,
      cachePathsExcludeExactOutputs(full),
      `${sourceFile} cache paths must exclude Cargo target and exact proof outputs`,
    );
    const compilerSaveIndex = stepIndex(full, "Save compiler objects after compilation");
    add(
      violations,
      compilerSaveIndex > stepIndex(full, "Lint every workspace target and feature once")
        && stepIndex(full, "Stop compilation clock")
          > stepIndex(full, "Lint every workspace target and feature once")
        && stepIndex(full, "Stop compilation clock") < compilerSaveIndex
        && stepIndex(full, "Start compiler cache save clock")
          > stepIndex(full, "Save Cargo dependency inputs")
        && stepIndex(full, "Start compiler cache save clock") < compilerSaveIndex
        && compilerSaveIndex < stepIndex(full, "Test the complete workspace once")
        && compilerSaveIndex < stepIndex(full, "Emit authenticated source release cell"),
      `${sourceFile} compiler cache must save before test execution or release-cell failure`,
    );
    requireStepRun(violations, sourceFile, full, "Compile the complete workspace test suite", [
      "cargo test --workspace --locked --no-run",
    ]);
    requireStepRun(violations, sourceFile, full, "Report compiler cache restore", [
      "--requested-key",
      "--matched-key",
      "--compatibility-prefix",
      "--cache-hit",
      "--path \"$SCCACHE_DIR\"",
    ]);
    requireStepRun(violations, sourceFile, full, "Report compiler cache save", [
      "--restored-bytes",
      "--started-ms",
      "--ended-ms",
      "--save-started-ms",
      "--save-result",
      "--path \"$SCCACHE_DIR\"",
    ]);
    requireStepRun(violations, sourceFile, full, "Require successful source compilation", [
      'test "$COMPILE_OUTCOME" = success',
      'test "$LINT_OUTCOME" = success',
    ]);
    const compile = namedStep(full, "Compile the complete workspace test suite");
    const lint = namedStep(full, "Lint every workspace target and feature once");
    add(
      violations,
      compile?.["continue-on-error"] === true
        && lint?.["continue-on-error"] === true
        && lint?.if === "steps.compile-workspace.outcome == 'success'",
      `${sourceFile} compilation and lint must preserve cache state before reporting failure`,
    );
    // nextest owns unit/integration execution; the doc pass rides along so a future doctest can
    // never silently stop being run (nextest does not execute doctests).
    requireStepRun(violations, sourceFile, full, "Test the complete workspace once", [
      "cargo nextest run --workspace --locked",
      "cargo test --workspace --doc --locked",
    ]);
    requireStepRun(violations, sourceFile, full, "Install pinned cargo-nextest", [
      `cargo-nextest-${nextestVersion}-x86_64-unknown-linux-gnu.tar.gz`,
      `${nextestLinuxSha256}  $RUNNER_TEMP/cargo-nextest.tar.gz`,
      "sha256sum --check --strict",
      "cargo nextest --version",
    ]);
    add(
      violations,
      stepIndex(full, "Install pinned cargo-nextest") > stepIndex(full, "Install pinned sccache")
        && stepIndex(full, "Install pinned cargo-nextest")
          < stepIndex(full, "Test the complete workspace once"),
      `${sourceFile} must install the pinned test runner before the workspace test step`,
    );
    requireStepRun(violations, sourceFile, full, "Lint every workspace target and feature once", ["cargo clippy --workspace --all-targets --all-features --locked -- -D warnings"]);
    requireStepRun(violations, sourceFile, full, "Emit authenticated source release cell", [
      "codestory-release-cell-manifest.mjs produce",
      "--cell-id source_behavior",
      "--producer-job full-source-gate",
    ]);
    const sourceCellUpload = namedStep(full, "Upload authenticated source release cell");
    add(
      violations,
      sourceCellUpload?.uses === "actions/upload-artifact@v7.0.1"
        && String(sourceCellUpload?.if ?? "").includes("success()")
        && String(sourceCellUpload?.if ?? "").includes("inputs.emit_release_cells"),
      `${sourceFile} source release cell must be a success-only retained artifact`,
    );
  }
}

function validateReleaseCoordinator(workflows, violations, graph) {
  const releaseChain = graph.workflow_policy.release_chain;
  const releaseFile = "release.yml";
  const release = workflows.get(releaseFile);
  if (!release) {
    violations.push(`${releaseFile} must exist`);
    return;
  }
  const releaseCallers = [...workflows.entries()]
    .filter(([file, workflow]) => file !== releaseFile
      && scalarStrings(workflow).some(value => value === "./.github/workflows/release.yml"))
    .map(([file]) => file)
    .sort();
  add(
    violations,
    JSON.stringify(releaseCallers) === JSON.stringify(["auto-release.yml"]),
    `${releaseFile} publication authority must have only the trusted auto-release.yml caller`,
  );
  add(violations, object(release.permissions).actions === "read", `${releaseFile} must read prior-run evidence`);
  add(
    violations,
    object(release.permissions)["pull-requests"] === "read",
    `${releaseFile} must grant the exact source resolver pull-request metadata access`,
  );
  const callExpectedHead = object(at(release, "on", "workflow_call", "inputs", "expected_head_sha"));
  add(
    violations,
    callExpectedHead.required === false && callExpectedHead.type === "string" && callExpectedHead.default === "",
    `${releaseFile} workflow_call expected_head_sha must be an optional empty string`,
  );
  const callPublish = object(at(release, "on", "workflow_call", "inputs", "publish_release"));
  add(
    violations,
    callPublish.required === false && callPublish.type === "boolean" && callPublish.default === false,
    `${releaseFile} workflow_call publish_release must be a fail-closed boolean`,
  );
  const dispatchExpectedHead = object(at(release, "on", "workflow_dispatch", "inputs", "expected_head_sha"));
  add(
    violations,
    dispatchExpectedHead.required === true && dispatchExpectedHead.type === "string",
    `${releaseFile} workflow_dispatch expected_head_sha must be a required string`,
  );
  add(
    violations,
    at(release, "on", "workflow_dispatch", "inputs", "publish_release") === undefined,
    `${releaseFile} workflow_dispatch must not expose publication authority`,
  );
  requireNoCalibrationReferences(violations, releaseFile, release);
  const policy = requireJob(violations, releaseFile, release, "workflow-policy");
  requireStepRun(violations, releaseFile, policy, "Install workflow policy dependencies", ["npm ci --ignore-scripts"]);
  requireStepRun(violations, releaseFile, policy, "Check workflow syntax", [
    "node --test .github/scripts/run-actionlint.test.mjs",
    "node .github/scripts/run-actionlint.mjs",
  ]);
  requireStepRun(violations, releaseFile, policy, "Check release claim and evidence contracts", [
    "scripts/tests/codestory-release-claims.test.mjs",
    "scripts/tests/codestory-release-cell-manifest.test.mjs",
    "scripts/tests/codestory-release-closeout.test.mjs",
    "scripts/tests/codestory-release-evidence-gate.test.mjs",
  ]);
  requireStepRun(violations, releaseFile, policy, "Enforce workflow policy", ["node .github/scripts/check-workflow-policy.mjs"]);

  const preflight = requireJob(violations, releaseFile, release, "preflight");
  add(violations, sameMembers(needs(preflight), releaseChain.dependencies.preflight), `${releaseFile} preflight dependencies must match the release claim graph`);
  requireStepRun(violations, releaseFile, preflight, "Validate release authority", [
    'if [ "$PUBLISH_RELEASE" = "true" ]; then',
    '"$GITHUB_EVENT_NAME" != "push"',
    '"$GITHUB_REF" != "refs/heads/main"',
    '$GITHUB_REPOSITORY/.github/workflows/auto-release.yml@refs/heads/main',
    '"$GITHUB_WORKFLOW_REF" != "$expected_caller"',
    'repos/$GITHUB_REPOSITORY/git/ref/heads/main',
    '"$GITHUB_EVENT_NAME" != "workflow_dispatch"',
    '"$GITHUB_REF" != "refs/heads/dev/codestory-next"',
    '"$EXPECTED_HEAD_SHA" != "$GITHUB_SHA"',
    'repos/$GITHUB_REPOSITORY/git/ref/heads/dev/codestory-next',
    "dev/codestory-next moved from proved head",
  ]);
  requireStepRun(violations, releaseFile, preflight, "Validate versioned changelog notes", [
    "node .github/scripts/extract-codestory-release-notes.mjs",
    '--version "$VERSION"',
  ]);
  requireStepRun(violations, releaseFile, preflight, "Refuse existing tag or release", [
    'git ls-remote --exit-code --tags origin "refs/tags/$TAG"',
    'gh release view "$TAG"',
    "exit 1",
  ]);
  const marketplacePreflight = namedStep(
    preflight,
    "Prove the public marketplace install path",
  );
  add(
    violations,
    marketplacePreflight?.if === "inputs.publish_release"
      && marketplacePreflight?.["continue-on-error"] === undefined,
    `${releaseFile} public marketplace preflight must run before publication and fail closed`,
  );
  requireStepRun(
    violations,
    releaseFile,
    preflight,
    "Prove the public marketplace install path",
    [
      "git ls-remote",
      "refs/heads/main",
      '"@openai/codex@$CODEX_CLI_VERSION"',
      "install-codestory-marketplace-proof.mjs",
      '--source-repository "$GITHUB_WORKSPACE"',
      "marketplace_revision=$marketplace_revision",
    ],
  );
  add(
    violations,
    object(preflight.outputs).marketplace_revision
      === "${{ steps.marketplace.outputs.marketplace_revision }}",
    `${releaseFile} preflight must publish the proved immutable marketplace revision`,
  );

  const source = requireJob(violations, releaseFile, release, "source-proof");
  add(violations, source.uses === "./.github/workflows/source-proof.yml", `${releaseFile} must call exact source proof`);
  add(violations, sameMembers(needs(source), releaseChain.dependencies["source-proof"]), `${releaseFile} source proof dependencies must match the release claim graph`);
  add(violations, object(source.with).ref === "${{ github.sha }}", `${releaseFile} source proof must receive the exact release SHA`);
  // Reuse is admissible only through the authenticated closeout binding, never by simply
  // dropping the gate: the job may be skipped, and only when preflight resolved reusable
  // evidence for this exact tree.
  add(
    violations,
    String(source.if ?? "") === "needs.preflight.outputs.source_proof_reused != 'true'",
    `${releaseFile} source proof may be skipped only when preflight resolved reusable evidence`,
  );
  requireStepRun(violations, releaseFile, requireJob(violations, releaseFile, release, "preflight"), "Resolve reusable prior evidence", [
    'git rev-parse "$GITHUB_SHA^{tree}"',
    "merge-base --is-ancestor",
    "full-source-gate",
    '.path == ".github/workflows/source-proof.yml"',
  ]);
  const closeout = requireJob(violations, releaseFile, release, "pre-publish-closeout");
  requireStepRun(violations, releaseFile, closeout, "Authenticate pre-publish Actions provenance", [
    '--reuse "$REUSE_SELECTION"',
  ]);
  add(
    violations,
    String(closeout.if ?? "").includes("needs.source-proof.result == 'skipped'")
      && String(closeout.if ?? "").includes("needs.preflight.result == 'success'"),
    `${releaseFile} closeout must accept a skipped source gate only alongside a successful preflight`,
  );
  add(violations, object(source.with).version === "${{ needs.preflight.outputs.version }}" && object(source.with).emit_release_cells === true, `${releaseFile} source proof must emit its authenticated release cell`);

  const packaged = requireJob(violations, releaseFile, release, "packaged-proof");
  add(violations, packaged.uses === "./.github/workflows/packaged-platform-proof.yml", `${releaseFile} packaged-proof must call the package workflow`);
  add(violations, sameMembers(needs(packaged), releaseChain.dependencies["packaged-proof"]), `${releaseFile} packaged-proof dependencies must match the release claim graph`);
  add(violations, object(packaged.with).sign_macos === true, `${releaseFile} packaged-proof must sign Mac assets`);
  add(violations, object(packaged.with).emit_release_cells === true, `${releaseFile} packaged-proof must emit all package release cells`);
  add(
    violations,
    object(packaged.with).hermetic_linux === false,
    `${releaseFile} main release must not repeat frozen-candidate Linux qualification`,
  );
  add(
    violations,
    object(packaged.with).scope === "full",
    `${releaseFile} packaged-proof must build the graph-declared release targets`,
  );
  for (const key of ["calibration_bundle_artifact", "calibration_bundle_run_id"]) {
    add(
      violations,
      object(packaged.with)[key] === undefined,
      `${releaseFile} packaged proof must not receive ${key}`,
    );
  }
  const expectedSecrets = [
    "APPLE_DEVELOPER_ID_P12_BASE64",
    "APPLE_DEVELOPER_ID_P12_PASSWORD",
    "APPLE_SIGNING_IDENTITY",
    "APPLE_NOTARY_KEY_P8_BASE64",
    "APPLE_NOTARY_KEY_ID",
    "APPLE_NOTARY_ISSUER_ID",
  ];
  add(violations, sameMembers(Object.keys(object(packaged.secrets)), expectedSecrets), `${releaseFile} packaged-proof must pass exactly the Apple signing secrets`);

  add(
    violations,
    at(release, "jobs", "release-evidence") === undefined,
    `${releaseFile} must not make optional performance or answer-quality evaluation release-blocking`,
  );

  const metal = requireJob(violations, releaseFile, release, "macos-metal-proof");
  add(violations, metal.uses === "./.github/workflows/macos-metal-proof.yml", `${releaseFile} must call protected Metal proof`);
  add(violations, sameMembers(needs(metal), releaseChain.dependencies["macos-metal-proof"]), `${releaseFile} Metal proof dependencies must match the release claim graph`);
  add(violations, object(metal.with).use_packaged_cli_artifact === true, `${releaseFile} Metal proof must use the packaged CLI`);
  add(violations, object(metal.with).emit_release_cells === true, `${releaseFile} Metal proof must emit its authenticated release cell`);
  add(
    violations,
    object(metal.with).candidate_installed_proof === true
      && object(metal.with).candidate_installed_only === undefined
      && object(metal.with).server_behavior_only === true
      && object(metal.with).quality_evidence_artifact === undefined,
    `${releaseFile} Mac proof must close Metal and candidate-installed claims without optional quality evidence`,
  );
  requireStepRun(violations, "macos-metal-proof.yml", at(workflows.get("macos-metal-proof.yml"), "jobs", "packaged-metal"), "Emit authenticated macOS retrieval-readiness release cell", [
    "--cell-id retrieval_readiness:macos-arm64",
    "release-cell-postpublish-retrieval-macos-arm64-attempt-$GITHUB_RUN_ATTEMPT",
  ]);

  const vulkan = requireJob(violations, releaseFile, release, "windows-vulkan-proof");
  add(violations, vulkan.uses === "./.github/workflows/windows-vulkan-proof.yml", `${releaseFile} must call protected Vulkan proof`);
  add(violations, sameMembers(needs(vulkan), releaseChain.dependencies["windows-vulkan-proof"]), `${releaseFile} Vulkan proof dependencies must match the release claim graph`);
  add(violations, object(vulkan.with).use_packaged_cli_artifact === true, `${releaseFile} Vulkan proof must use the packaged CLI`);
  add(violations, object(vulkan.with).emit_release_cells === true, `${releaseFile} Vulkan proof must emit its authenticated release cell`);
  add(
    violations,
    object(vulkan.with).candidate_installed_proof === true
      && object(vulkan.with).candidate_installed_only === undefined
      && object(vulkan.with).server_behavior_only === true
      && object(vulkan.with).quality_evidence_artifact === undefined,
    `${releaseFile} Windows proof must close Vulkan and candidate-installed claims without optional quality evidence`,
  );
  requireStepRun(violations, "windows-vulkan-proof.yml", at(workflows.get("windows-vulkan-proof.yml"), "jobs", "packaged-vulkan"), "Emit authenticated Windows retrieval-readiness release cell", [
    "--cell-id retrieval_readiness:windows-x64",
    "release-cell-postpublish-retrieval-windows-x64-attempt-$GITHUB_RUN_ATTEMPT",
  ]);

  const linuxVulkan = requireJob(violations, releaseFile, release, "linux-vulkan-proof");
  add(violations, linuxVulkan.uses === "./.github/workflows/linux-vulkan-proof.yml", `${releaseFile} must call protected Linux Vulkan proof`);
  add(violations, sameMembers(needs(linuxVulkan), releaseChain.dependencies["linux-vulkan-proof"]), `${releaseFile} Linux Vulkan proof dependencies must match the release claim graph`);
  add(
    violations,
    object(linuxVulkan.with).candidate_installed_proof === true
      && object(linuxVulkan.with).server_behavior_only === true
      && object(linuxVulkan.with).emit_release_cells === true,
    `${releaseFile} Linux proof must close Vulkan, retrieval, and candidate-installed claims`,
  );
  requireStepRun(violations, "linux-vulkan-proof.yml", at(workflows.get("linux-vulkan-proof.yml"), "jobs", "packaged-vulkan"), "Emit authenticated Linux Vulkan release cells", [
    "--cell-id accelerator_execution:linux-x64-vulkan",
    "--cell-id retrieval_readiness:linux-x64",
    "--cell-id candidate_installed_behavior:linux-x64",
  ]);

  const preCloseout = requireJob(violations, releaseFile, release, "pre-publish-closeout");
  add(violations, sameMembers(needs(preCloseout), releaseChain.dependencies["pre-publish-closeout"]), `${releaseFile} pre-publish closeout dependencies must match the release claim graph`);
  requireStepRun(violations, releaseFile, preCloseout, "Authenticate pre-publish Actions provenance", [
    "producer-map",
    "--phase pre_publish",
    "artifact_ids",
  ]);
  const preDownload = namedStep(preCloseout, "Download selected pre-publish release cells");
  add(
    violations,
    preDownload?.uses === "actions/download-artifact@v8.0.1"
      && object(preDownload.with)["artifact-ids"] === "${{ steps.pre-publish-provenance.outputs.artifact_ids }}"
      && object(preDownload.with)["merge-multiple"] === false,
    `${releaseFile} pre-publish closeout must download selected Actions artifact ids without flattening`,
  );
  requireStepRun(violations, releaseFile, preCloseout, "Verify selected pre-publish artifact container digests", [
    "/actions/artifacts/$artifact_id/zip",
    "sha256sum",
    "test \"$actual_digest\" = \"$expected_digest\"",
  ]);
  requireStepRun(violations, releaseFile, preCloseout, "Evaluate authenticated pre-publish closeout", [
    "--trusted-producers",
    "codestory-release-closeout.mjs evaluate",
  ]);
  const devRevalidation = namedStep(preCloseout, "Revalidate proof-only dev head");
  add(
    violations,
    devRevalidation?.if === "${{ !inputs.publish_release }}",
    `${releaseFile} proof-only ledger upload must revalidate only the dev head`,
  );
  requireStepRun(violations, releaseFile, preCloseout, "Revalidate proof-only dev head", [
    "repos/$GITHUB_REPOSITORY/git/ref/heads/dev/codestory-next",
    '"$live_head" != "$GITHUB_SHA"',
    "dev/codestory-next moved from accepted ledger head",
  ]);
  requireStepUses(violations, releaseFile, preCloseout, "Upload accepted pre-publish closeout", "actions/upload-artifact@v7.0.1");

  const publish = requireJob(violations, releaseFile, release, "publish");
  add(violations, publish.if === "inputs.publish_release", `${releaseFile} publish must require trusted publication authority`);
  add(violations, sameMembers(needs(publish), releaseChain.dependencies.publish), `${releaseFile} publish dependencies must match the release claim graph`);
  requireStepRun(violations, releaseFile, publish, "Compose versioned GitHub release notes", [
    "node .github/scripts/extract-codestory-release-notes.mjs",
    "--output target/release-assets/release-notes.md",
    "node scripts/codestory-release-claims.mjs release-platform-notes",
  ]);
  requireStepRun(violations, releaseFile, publish, "Refuse existing tag or release", [
    'git ls-remote --exit-code --tags origin "refs/tags/$TAG"',
    'gh release view "$TAG"',
    "exit 1",
  ]);
  requireStepRun(violations, releaseFile, publish, "Create GitHub release", [
    "--notes-file target/release-assets/release-notes.md",
    "node scripts/codestory-release-claims.mjs release-assets",
    'for name in "${expected_names[@]}"; do',
    'if [ ! -f "$asset" ]; then',
    "Release assets differ from the release claim graph",
    "repos/$GITHUB_REPOSITORY/git/ref/heads/main",
    '"$live_head" != "$GITHUB_SHA"',
    "main moved from publishable head",
  ]);
  add(violations, !scalarStrings(release).some(value => value.includes("--generate-notes")), `${releaseFile} must use curated release notes`);

  const post = requireJob(violations, releaseFile, release, "post-publish-smoke");
  add(violations, post.if === "inputs.publish_release", `${releaseFile} post-publish smoke must require trusted publication authority`);
  add(violations, post.uses === "./.github/workflows/post-publish-release-smoke.yml", `${releaseFile} must call post-publish smoke`);
  add(violations, sameMembers(needs(post), releaseChain.dependencies["post-publish-smoke"]), `${releaseFile} post-publish dependencies must match the release claim graph`);
  add(
    violations,
    object(post.with).emit_release_cells === true
      && object(post.with).marketplace_revision
        === "${{ needs.preflight.outputs.marketplace_revision }}"
      && String(object(post.with).pre_publish_closeout_artifact ?? "").startsWith("release-closeout-pre-publish-"),
    `${releaseFile} post-publish smoke must consume the proved marketplace revision and accepted pre-publish ledger`,
  );
  const postCloseout = requireJob(violations, releaseFile, release, "post-publish-closeout");
  add(violations, postCloseout.if === "inputs.publish_release", `${releaseFile} post-publish closeout must require trusted publication authority`);
  add(violations, sameMembers(needs(postCloseout), releaseChain.dependencies["post-publish-closeout"]), `${releaseFile} post-publish closeout dependencies must match the release claim graph`);
  requireStepRun(violations, releaseFile, postCloseout, "Authenticate post-publish Actions provenance", [
    "producer-map",
    "--phase post_publish",
    "artifact_ids",
  ]);
  const postDownload = namedStep(postCloseout, "Download selected release cells without flattening");
  add(
    violations,
    postDownload?.uses === "actions/download-artifact@v8.0.1"
      && object(postDownload.with)["artifact-ids"] === "${{ steps.post-publish-provenance.outputs.artifact_ids }}"
      && object(postDownload.with)["merge-multiple"] === false,
    `${releaseFile} post-publish closeout must download selected Actions artifact ids without flattening`,
  );
  requireStepRun(violations, releaseFile, postCloseout, "Verify selected post-publish artifact container digests", [
    "/actions/artifacts/$artifact_id/zip",
    "sha256sum",
    "test \"$actual_digest\" = \"$expected_digest\"",
  ]);
  requireStepRun(violations, releaseFile, postCloseout, "Evaluate authenticated post-publish closeout", [
    "--trusted-producers",
    "--pre-publish-ledger",
    "codestory-release-closeout.mjs evaluate",
  ]);
  requireStepUses(violations, releaseFile, postCloseout, "Upload accepted post-publish closeout", "actions/upload-artifact@v7.0.1");
  for (const [jobName, job] of [
    ["Metal proof", metal],
    ["Windows Vulkan proof", vulkan],
    ["Linux Vulkan proof", linuxVulkan],
    ["post-publish proof", post],
  ]) {
    for (const key of ["calibration_bundle_artifact", "calibration_bundle_run_id"]) {
      add(
        violations,
        object(job.with)[key] === undefined,
        `${releaseFile} ${jobName} must not receive ${key}`,
      );
    }
  }
}

function expectedPackageRows(graph) {
  return graph.workflow_policy.package_matrix;
}

function expectedPostPublishRows() {
  return [
    {
      asset_target: "windows-x64",
      runs_on: '["self-hosted","Windows","X64","codestory-vulkan"]',
      environment: "windows-vulkan-proof",
      backend: "Vulkan",
      extension: "zip",
    },
    {
      asset_target: "macos-arm64",
      runs_on: '["self-hosted","macOS","ARM64","codestory-metal"]',
      environment: "macos-metal-release",
      backend: "Metal",
      extension: "tar.gz",
    },
    {
      asset_target: "linux-x64",
      runs_on: '["self-hosted","Linux","X64","codestory-linux-vulkan"]',
      environment: "linux-vulkan-proof",
      backend: "Vulkan",
      extension: "tar.gz",
    },
  ];
}

function validatePackageMatrixExpression(violations, expression, graph) {
  const match = typeof expression === "string" && expression.match(
    /fromJSON\(inputs\.calibration_mode && '([^']+)' \|\| inputs\.scope == 'linux' && '([^']+)' \|\| inputs\.scope == 'windows' && '([^']+)' \|\| inputs\.scope == 'macos' && '([^']+)' \|\| '([^']+)'\)/u,
  );
  if (!match) {
    violations.push("packaged-platform-proof.yml matrix must select structural JSON by scope");
    return;
  }
  const linuxX64 = graph.workflow_policy.package_matrix.find(({ asset_target: target }) =>
    target === "linux-x64");
  const windowsX64 = graph.workflow_policy.package_matrix.find(({ asset_target: target }) =>
    target === "windows-x64");
  const macosArm64 = graph.workflow_policy.package_matrix.find(({ asset_target: target }) =>
    target === "macos-arm64");
  const expected = [
    { include: [linuxX64] },
    { include: [linuxX64] },
    { include: [windowsX64] },
    { include: [macosArm64] },
    { include: expectedPackageRows(graph) },
  ];
  try {
    match.slice(1).forEach((json, index) => {
      add(violations, JSON.stringify(JSON.parse(json)) === JSON.stringify(expected[index]), "packaged-platform-proof.yml package matrix scope changed");
    });
  } catch {
    violations.push("packaged-platform-proof.yml package matrix must contain valid JSON");
  }
}

function validatePackagedProof(workflows, violations, graph) {
  const file = "packaged-platform-proof.yml";
  const workflow = workflows.get(file);
  if (!workflow) {
    violations.push(`${file} must exist`);
    return;
  }
  add(violations, trigger(workflow, "workflow_call") !== undefined, `${file} must be reusable`);
  const refInput = object(at(workflow, "on", "workflow_call", "inputs", "ref"));
  add(
    violations,
    refInput.required === true && refInput.type === "string" && refInput.default === undefined,
    `${file} must require one exact source SHA`,
  );
  add(violations, object(workflow.permissions).contents === "read", `${file} must use read-only contents permission`);
  add(violations, object(workflow.permissions).actions === "read", `${file} must read authenticated qualification artifacts`);
  for (const key of ["calibration_bundle_artifact", "calibration_bundle_run_id"]) {
    requireOptionalStringInput(violations, file, workflow, "workflow_call", key);
  }
  for (const key of [
    "candidate_installed_proof",
    "candidate_installed_only",
    "server_behavior_only",
    "enforce_calibration_freeze_lineage",
  ]) {
    add(
      violations,
      at(workflow, "on", "workflow_call", "inputs", key) === undefined,
      `${file} package-only workflow must not define ${key}`,
    );
  }
  add(
    violations,
    object(workflow.env).LINUX_GLIBC_BUILD_IMAGE ===
      "rust:1.95.0-bullseye@sha256:28afaeb8445f2a2e7d878bd34ed39ba02bb517efb29986188cbd59b7cf4f2fdf",
    `${file} must pin the glibc build image`,
  );
  add(
    violations,
    object(workflow.env).LINUX_GLSLC_IMAGE ===
      "ubuntu:24.04@sha256:4fbb8e6a8395de5a7550b33509421a2bafbc0aab6c06ba2cef9ebffbc7092d90",
    `${file} must pin the glslc build image`,
  );
  const job = requireJob(violations, file, workflow, "build");
  add(
    violations,
    job["timeout-minutes"] === "${{ inputs.calibration_mode && 180 || (inputs.sign_macos && startsWith(matrix.asset_target, 'macos-') && 90 || 60) }}",
    `${file} package build timeout must cover only calibration or signed macOS packaging`,
  );
  add(
    violations,
    at(workflow, "concurrency", "group") === "packaged-platform-proof-${{ github.sha }}-${{ inputs.ref }}-${{ inputs.proof_key || github.ref }}",
    `${file} concurrency must bind caller SHA, exact package SHA, and proof identity`,
  );
  validatePackageMatrixExpression(violations, at(job, "strategy", "matrix"), graph);
  add(violations, String(job.environment ?? "").includes("macos-release-signing"), `${file} signed Mac cells must use the protected signing environment`);
  const packageSteps = list(job.steps).map(object);
  add(
    violations,
    object(workflow.env).SCCACHE_VERSION === sccacheVersion
      && object(workflow.env).SCCACHE_CACHE_SIZE === sccacheCacheSize
      && object(workflow.env).WINDOWS_SCCACHE_CACHE_SIZE === windowsSccacheCacheSize
      && object(workflow.env).CARGO_DEPENDENCY_CACHE_MAX_BYTES === "1073741824",
    `${file} must pin bounded compiler and dependency caches`,
  );
  const hermeticInput = object(at(workflow, "on", "workflow_call", "inputs", "hermetic_linux"));
  add(
    violations,
    hermeticInput.required === false
      && hermeticInput.default === false
      && hermeticInput.type === "boolean",
    `${file} frozen Linux qualification must be explicit and off by default`,
  );
  const shortWindowsTarget = namedStep(job, "Configure short Windows Cargo target");
  const checkout = namedStep(job, "Checkout");
  add(
    violations,
    checkout?.uses === "actions/checkout@v5"
      && object(checkout?.with).ref === "${{ inputs.ref }}",
    `${file} package jobs must checkout only the requested exact SHA`,
  );
  requireStepRun(violations, file, job, "Require exact source identity", [
    '[[ "$EXACT_SHA" =~ ^[0-9a-f]{40}$ ]]',
    'head_sha="$(git rev-parse HEAD)"',
    "source_tree=\"$(git rev-parse 'HEAD^{tree}')\"",
    'test "$head_sha" = "$EXACT_SHA"',
  ]);
  add(
    violations,
    shortWindowsTarget?.if === "runner.os == 'Windows'" && shortWindowsTarget?.shell === "pwsh",
    `${file} short Cargo target must be Windows-only PowerShell setup`,
  );
  const sccacheSetup = namedStep(job, "Install pinned sccache");
  add(
    violations,
    sccacheSetup?.uses === sccacheAction
      && object(sccacheSetup?.with).version === "${{ env.SCCACHE_VERSION }}",
    `${file} must install the pinned sccache action and binary`,
  );
  requireStepRun(violations, file, job, "Configure short Windows Cargo target", [
    '$workspaceTarget = Join-Path $env:GITHUB_WORKSPACE "target"',
    '$runnerRoot = [System.IO.Path]::GetPathRoot($workspaceTarget)',
    '[string]::IsNullOrWhiteSpace($runnerRoot)',
    'Join-Path $runnerRoot "t"',
    "New-Item -ItemType Junction -Path $shortTarget -Target $workspaceTarget",
    '"CARGO_TARGET_DIR=$shortTarget" | Out-File -FilePath $env:GITHUB_ENV',
  ]);
  requireStepRun(violations, file, job, "Configure bounded compiler cache", [
    'cache_size="$SCCACHE_CACHE_SIZE"',
    'if [[ "$RUNNER_OS" == "Windows" ]]',
    'cache_size="$WINDOWS_SCCACHE_CACHE_SIZE"',
    "CARGO_HOME=$RUNNER_TEMP/codestory-release-cargo",
    "SCCACHE_DIR=$RUNNER_TEMP/codestory-release-sccache",
    "SCCACHE_CACHE_SIZE=$cache_size",
    "RUSTC_WRAPPER=sccache",
    "CARGO_INCREMENTAL=0",
    "CMAKE_C_COMPILER_LAUNCHER=sccache",
    "CMAKE_CXX_COMPILER_LAUNCHER=sccache",
    "CMAKE_GENERATOR=Ninja",
  ]);
  const nativeIdentity = namedStep(job, "Capture reusable build cache contract");
  const nativeIdentityRun = executableRunText(String(nativeIdentity?.run ?? ""));
  add(
    violations,
    nativeIdentity?.id === "build-cache"
      && nativeIdentity?.shell === "bash"
      && object(nativeIdentity?.env).CALIBRATION_MODE === "${{ inputs.calibration_mode }}"
      && object(nativeIdentity?.env).QUALITY_EVIDENCE_ARTIFACT
        === "${{ inputs.quality_evidence_artifact }}"
      && nativeIdentityRun.includes(`--namespace ${graph.workflow_policy.promotion.packaged_cache_namespace}`)
      && nativeIdentityRun.includes('--exact-sha "$EXACT_SHA"')
      && nativeIdentityRun.includes('--os "$RUNNER_OS"')
      && nativeIdentityRun.includes('--target "${{ matrix.rust_target }}"')
      && nativeIdentityRun.includes('--rust-version "$rust_version"')
      && nativeIdentityRun.includes("--features codestory-cli-default-features")
      && nativeIdentityRun.includes("--native-toolchain")
      && nativeIdentityRun.includes("--generator")
      && nativeIdentityRun.includes("--cmake-version")
      && nativeIdentityRun.includes("--ninja-version")
      && nativeIdentityRun.includes("--sccache-version \"$SCCACHE_VERSION\"")
      && nativeIdentityRun.includes("--lock-file Cargo.lock")
      && nativeIdentityRun.includes("--cargo-config .cargo/config.toml")
      && nativeIdentityRun.includes("--identity cargo_incremental=0")
      && nativeIdentityRun.includes("qualification_driver=disabled")
      && nativeIdentityRun.includes("qualification_driver=enabled")
      && nativeIdentityRun.includes('--identity "qualification_driver=$qualification_driver"')
      && nativeIdentityRun.includes(".cargo/llama-dynamic-backends.cmake")
      && nativeIdentityRun.includes("git ls-files '*Cargo.toml'")
      && nativeIdentityRun.includes("model-contract.json")
      && nativeIdentityRun.includes("install-windows-vulkan-sdk.ps1")
      && nativeIdentityRun.includes("linux-glibc-build.Dockerfile")
      && nativeIdentityRun.includes(".github/docker/glslc")
      && nativeIdentityRun.includes("--identity cxxflags=-std=c++17")
      && nativeIdentityRun.includes("LINUX_GLIBC_BUILD_IMAGE")
      && nativeIdentityRun.includes("LINUX_GLSLC_IMAGE"),
    `${file} must compute one complete reusable compiler compatibility contract`,
  );
  const dependencyRestore = namedStep(job, "Restore Cargo dependency inputs");
  const dependencyRestoreWith = object(dependencyRestore?.with);
  add(
    violations,
    dependencyRestore?.uses === "actions/cache/restore@v5"
      && sameMembers(cachePaths(dependencyRestore), [
        "${{ runner.temp }}/codestory-release-cargo/registry",
        "${{ runner.temp }}/codestory-release-cargo/git",
      ])
      && dependencyRestoreWith.key === "${{ steps.build-cache.outputs.dependency-key }}"
      && dependencyRestoreWith["restore-keys"] === undefined,
    `${file} dependency cache must be exact-input-only and exclude compiler output`,
  );
  const compilerRestore = namedStep(job, "Restore compatible compiler objects");
  const compilerRestoreWith = object(compilerRestore?.with);
  add(
    violations,
    compilerRestore?.uses === "actions/cache/restore@v5"
      && sameMembers(cachePaths(compilerRestore), ["${{ runner.temp }}/codestory-release-sccache"])
      && compilerRestoreWith.key === "${{ steps.build-cache.outputs.compiler-key }}"
      && String(compilerRestoreWith["restore-keys"] ?? "").trim()
        === "${{ steps.build-cache.outputs.compiler-prefix }}",
    `${file} compiler cache must restore the newest compatible prior candidate`,
  );
  const dependencySave = namedStep(job, "Save Cargo dependency inputs");
  const compilerSave = namedStep(job, "Save compiler objects after compilation");
  requireStepRun(violations, file, job, "Bound Cargo dependency cache", [
    "--max-bytes \"$CARGO_DEPENDENCY_CACHE_MAX_BYTES\"",
    "--path \"$CARGO_HOME/registry\"",
    "--path \"$CARGO_HOME/git\"",
  ]);
  add(
    violations,
    dependencySave?.uses === "actions/cache/save@v5"
      && String(dependencySave?.if ?? "").includes("always()")
      && String(dependencySave?.if ?? "").includes("steps.linux-build.outcome == 'success'")
      && String(dependencySave?.if ?? "").includes("steps.package-build.outcome == 'success'")
      && String(dependencySave?.if ?? "")
        .includes("steps.cargo-dependency-cache-size.outputs.within-limit == 'true'")
      && object(dependencySave?.with).key
        === "${{ steps.cargo-dependency-cache.outputs.cache-primary-key }}"
      && sameMembers(cachePaths(dependencySave), [
        "${{ runner.temp }}/codestory-release-cargo/registry",
        "${{ runner.temp }}/codestory-release-cargo/git",
      ]),
    `${file} dependency cache must save immediately after successful compilation`,
  );
  add(
    violations,
    compilerSave?.uses === "actions/cache/save@v5"
      && String(compilerSave?.if ?? "").includes("always()")
      && String(compilerSave?.if ?? "").includes("steps.linux-build.outcome == 'success'")
      && String(compilerSave?.if ?? "").includes("steps.package-build.outcome == 'success'")
      && object(compilerSave?.with).key
        === "${{ steps.compiler-cache-restore.outputs.cache-primary-key }}"
      && sameMembers(cachePaths(compilerSave), ["${{ runner.temp }}/codestory-release-sccache"]),
    `${file} compiler cache must save a new exact-SHA suffix after successful compilation`,
  );
  add(
    violations,
    cachePathsExcludeExactOutputs(job),
    `${file} cache paths must exclude Cargo target, native seeds, models, proofs, and exact archives`,
  );
  const compilerSaveIndex = stepIndex(job, "Save compiler objects after compilation");
  for (const lateStep of [
    "Prove native workspace path identity",
    "Test immutable native staging on Windows",
    "Sign and notarize macOS CLI",
    "Package release asset",
    "Package release asset on Windows",
    "Smoke packaged release asset",
    "Smoke packaged release asset on Windows",
    "Upload release asset",
  ]) {
    add(
      violations,
      compilerSaveIndex >= 0 && compilerSaveIndex < stepIndex(job, lateStep),
      `${file} compiler cache must save before late ${lateStep} failure`,
    );
  }
  requireStepRun(violations, file, job, "Report compiler cache restore", [
    "--requested-key",
    "--matched-key",
    "--compatibility-prefix",
    "--cache-hit",
    "--path \"$SCCACHE_DIR\"",
  ]);
  requireStepRun(violations, file, job, "Report compiler cache save", [
    "--restored-bytes",
    "--started-ms",
    "--ended-ms",
    "--save-started-ms",
    "--save-result",
    "--path \"$SCCACHE_DIR\"",
  ]);
  requireStepRun(violations, file, job, "Compile immutable native staging regression on Windows", [
    "cargo test --release --locked",
    "--test native_staging",
    "--no-run",
  ]);
  requireStepRun(violations, file, job, "Compile native workspace path regression on Windows", [
    "cargo test --locked -p codestory-workspace repository_identity --no-run",
  ]);
  requireStepRun(violations, file, job, "Build Linux x64 at the glibc 2.31 baseline", [
    'mkdir -p "$CARGO_HOME" "$SCCACHE_DIR"',
    "RUSTC_WRAPPER=/sccache/sccache",
    "SCCACHE_DIR=/sccache/cache",
    "CMAKE_C_COMPILER_LAUNCHER=/sccache/sccache",
    "CMAKE_CXX_COMPILER_LAUNCHER=/sccache/sccache",
    "$SCCACHE_PATH:/sccache/sccache:ro",
    "$SCCACHE_DIR:/sccache/cache",
    "/sccache/sccache --stop-server",
  ]);
  const finalizeCompilerObjects = namedStep(job, "Finalize compiler objects");
  add(
    violations,
    String(finalizeCompilerObjects?.if ?? "")
      .includes("steps.linux-build.outcome == 'success'")
      && String(finalizeCompilerObjects?.if ?? "")
        .includes("steps.qualification-driver.outcome != 'skipped'")
      && String(finalizeCompilerObjects?.if ?? "")
        .includes("steps.package-build.outcome == 'success'"),
    `${file} must stop the compiler server that performed each selected build`,
  );
  add(
    violations,
    compilerSaveIndex > stepIndex(job, "Build codestory-cli")
      && compilerSaveIndex > stepIndex(job, "Build Linux x64 at the glibc 2.31 baseline")
      && compilerSaveIndex > stepIndex(job, "Build qualification driver"),
    `${file} compiler cache must save after every selected compilation step`,
  );
  add(
    violations,
    stepIndex(job, "Build pinned Linux toolchain image")
      < stepIndex(job, "Start compilation clock")
      && stepIndex(job, "Stop compilation clock")
        > stepIndex(job, "Build qualification driver")
      && stepIndex(job, "Stop compilation clock") < compilerSaveIndex
      && stepIndex(job, "Start compiler cache save clock")
        > stepIndex(job, "Save Cargo dependency inputs")
      && stepIndex(job, "Start compiler cache save clock") < compilerSaveIndex,
    `${file} compile and compiler-cache-save timings must cover only their named stages`,
  );
  const linuxBuildDockerfile = fs.readFileSync(
    path.join(repositoryRoot, ".github", "docker", "linux-glibc-build.Dockerfile"),
    "utf8",
  );
  for (const fragment of [
    "clang-13=1:13.0.1-6~deb11u1",
    "libclang-13-dev=1:13.0.1-6~deb11u1",
    "CC=clang-13",
    "CXX=clang++-13",
    "LIBCLANG_PATH=/usr/lib/llvm-13/lib",
    "-mavxvnni -mavx512bf16 -mamx-tile -mamx-int8",
  ]) {
    add(
      violations,
      linuxBuildDockerfile.includes(fragment),
      `${file} Bullseye native build must preserve compiler contract ${fragment}`,
    );
  }
  const packageBuild = namedStep(job, "Build codestory-cli");
  add(
    violations,
    packageBuild?.env === undefined,
    `${file} native package build must not override the selected generator`,
  );
  requireStepRun(violations, file, job, "Smoke codestory-cli on Windows", [
    "$env:CARGO_TARGET_DIR",
    "${{ matrix.rust_target }}/release/codestory-cli",
  ]);
  requireStepRun(violations, file, job, "Package release asset on Windows", [
    "$env:CARGO_TARGET_DIR",
    "${{ matrix.rust_target }}/release/codestory-cli",
    "package-codestory-release.py",
    "--binary $bin",
  ]);
  requireStepRun(violations, file, job, "Prepare checksum-pinned embedded model", [
    "node scripts/prepare-embedded-model.mjs",
  ]);
  requireStepRun(violations, file, job, "Install Linux Vulkan build dependencies", [
    "bash .github/scripts/install-linux-vulkan-build-deps.sh",
  ]);
  const windowsNativeStagingTest = namedStep(job, "Test immutable native staging on Windows");
  add(
    violations,
    windowsNativeStagingTest?.if === "runner.os == 'Windows'",
    `${file} immutable native staging regression must run on Windows`,
  );
  requireStepRun(violations, file, job, "Test immutable native staging on Windows", [
    "cargo test --release --locked",
    "-p codestory-llama-sys",
    "--test native_staging",
    '--target "${{ matrix.rust_target }}"',
    "stages_complete_immutable_native_seeds",
  ]);
  requireStepRun(violations, file, job, "Build pinned Linux toolchain image", [
    ".github/docker/linux-glibc-build.Dockerfile",
    "LINUX_GLIBC_BUILD_IMAGE",
    "LINUX_GLSLC_IMAGE",
  ]);
  requireStepRun(violations, file, job, "Build Linux x64 at the glibc 2.31 baseline", [
    "cargo build --release --locked -p codestory-cli",
    "CARGO_TARGET_DIR=/workspace/target/glibc-2.31",
    "CXXFLAGS=-std=c++17",
  ]);
  for (const smokeStep of [
    "Smoke packaged release asset",
    "Smoke packaged release asset on Windows",
  ]) {
    requireStepRun(violations, file, job, smokeStep, [
      '--expected-source-sha "${{ steps.source-identity.outputs.sha }}"',
      '--expected-source-tree "${{ steps.source-identity.outputs.tree }}"',
    ]);
  }
  requireStepRun(violations, file, job, "Report fresh package identity", [
    "archive_sha256=",
    "Source SHA:",
    "Source tree:",
    "Archive SHA-256:",
  ]);
  add(
    violations,
    stepIndex(job, "Report fresh package identity")
      > stepIndex(job, "Smoke packaged release asset")
      && stepIndex(job, "Report fresh package identity")
        > stepIndex(job, "Smoke packaged release asset on Windows")
      && stepIndex(job, "Report fresh package identity")
        < stepIndex(job, "Upload release asset"),
    `${file} must report a verified fresh archive identity before upload`,
  );
  add(
    violations,
    namedStep(job, "Prove fresh-target Node-absent network-denied Cargo release boundary") === undefined,
    `${file} matrix package jobs must not repeat the frozen Linux Cargo boundary`,
  );
  const frozenLinux = requireJob(violations, file, workflow, "frozen-linux-qualification");
  add(
    violations,
    frozenLinux.if === "inputs.hermetic_linux"
      && sameMembers(needs(frozenLinux), ["build"])
      && frozenLinux["runs-on"] === "ubuntu-latest",
    `${file} frozen Linux Cargo boundary must be one explicit post-package job`,
  );
  const frozenCheckout = namedStep(frozenLinux, "Checkout frozen candidate");
  add(
    violations,
    frozenCheckout?.uses === "actions/checkout@v5"
      && object(frozenCheckout?.with).ref === "${{ inputs.ref }}",
    `${file} frozen Linux qualification must checkout the exact candidate`,
  );
  requireStepRun(
    violations,
    file,
    frozenLinux,
    "Prove fresh-target Node-absent network-denied Cargo release boundary",
    [
      "CARGO_HOME=\"$proof_root/cargo\"",
      "cargo fetch --locked",
      "--network none",
      "--read-only",
      "command -v node",
      "test ! -e \"$CARGO_TARGET_DIR\"",
      "cargo check --release --locked --offline -p codestory-llama-sys",
      "cargo build --release --locked --offline -p codestory-llama-sys",
    ],
  );
  add(
    violations,
    cacheSteps(frozenLinux).length === 0,
    `${file} frozen Linux fresh-target qualification must not restore compiler output`,
  );
  const signing = namedStep(job, "Sign and notarize macOS CLI");
  add(violations, signing !== undefined, `${file} must sign and notarize Mac binaries`);
  violations.push(...notaryStepViolations(signing).map(message => `${file} ${message}`));
  const signingRun = executableRunText(String(signing?.run ?? ""));
  for (const fragment of [
    "umask 077",
    "chmod 600",
    "--options runtime",
    "--timestamp",
    "xcrun notarytool submit",
    "--no-wait",
    "xcrun notarytool info",
    "xcrun notarytool log",
    'jq -e \'.status == "Accepted"\'',
    "TeamIdentifier=${APPLE_DEVELOPER_TEAM_ID}",
    "certificate leaf",
  ]) {
    add(violations, signingRun.includes(fragment), `${file} signing step must include ${fragment}`);
  }
  violations.push(...macosCliDistributionViolations(
    signing,
    namedStep(job, "Execute quarantined notarized macOS CLI without signing credentials"),
    '"$work_dir/codestory-cli-quarantined"',
  ).map(message => `${file} ${message}`));
  requireStepRun(violations, file, job, "Run Windows installer ownership self-test", ["scripts/install-codestory.ps1 -SelfTest"]);
  const linuxBaseline = namedStep(job, "Prove Linux x64 glibc 2.31 baseline");
  requireStepRun(violations, file, job, "Prove Linux x64 glibc 2.31 baseline", [
    "bash .github/scripts/check-linux-glibc-baseline.sh",
  ]);
  add(
    violations,
    !executableRunText(String(linuxBaseline?.run ?? "")).includes("libvulkan"),
    `${file} Linux glibc baseline must not install a Vulkan loader`,
  );
  const qualificationDriver = namedStep(job, "Build qualification driver");
  add(
    violations,
    qualificationDriver?.if
      === "matrix.asset_target == 'linux-x64' && (inputs.calibration_mode || inputs.quality_evidence_artifact != '')",
    `${file} qualification driver must skip the standard server-behavior path`,
  );
  requireCalibrationProducerBoundary(
    violations,
    file,
    job,
    "matrix.asset_target == 'linux-x64' && !inputs.calibration_mode && inputs.quality_evidence_artifact != ''",
  );
  requireStepRun(
    violations,
    file,
    job,
    "Packaged per-user server calibration or qualification",
    [
      "--proof-tier hosted_package",
      "calibration-bundle.json",
      '--calibration-bundle "$calibration_bundle"',
      "--calibration-producer-run-id",
      "--calibration-producer-artifact",
      'test -f "$quality_path"',
      "--engine-policy cpu_explicit",
      "--expected-backend CPU",
      "--produce-qualification-evidence",
      "--timeout-secs 1800",
    ],
  );
  const packagedProofRun = stepRun(
    job,
    "Packaged per-user server calibration or qualification",
  );
  const packagedProof = namedStep(
    job,
    "Packaged per-user server calibration or qualification",
  );
  const hostedCalibrationUpload = namedStep(job, "Upload hosted Linux calibration runs");
  add(
    violations,
    hostedCalibrationUpload?.uses === "actions/upload-artifact@v7.0.1"
      && hostedCalibrationUpload?.if
        === "success() && matrix.asset_target == 'linux-x64' && inputs.calibration_mode",
    `${file} hosted calibration artifact must remain calibration-only`,
  );
  const hostedEvaluationUpload = namedStep(job, "Upload packaged agent proof artifacts");
  add(
    violations,
    hostedEvaluationUpload?.uses === "actions/upload-artifact@v7.0.1"
      && String(hostedEvaluationUpload?.if ?? "").replace(/\s+/gu, " ")
        === "always() && matrix.asset_target == 'linux-x64' && (inputs.calibration_mode || inputs.quality_evidence_artifact != '')",
    `${file} hosted evaluation artifact must require explicit calibration or quality evidence`,
  );
  add(
    violations,
    String(packagedProof?.if ?? "").replace(/\s+/gu, " ")
      === "matrix.asset_target == 'linux-x64' && (inputs.calibration_mode || inputs.quality_evidence_artifact != '')",
    `${file} hosted CPU evaluation must require explicit calibration or quality evidence`,
  );
  add(
    violations,
    packagedProofRun.includes('if [ "$CALIBRATION_MODE" = true ]')
      && packagedProofRun.includes("--proof-tier calibration")
      && packagedProofRun.includes("--proof-tier hosted_package")
      && occurrenceCount(packagedProofRun, "--calibration-bundle") === 1
      && !packagedProofRun.includes("--server-behavior-only")
      && !packagedProofRun.includes("--ground-only")
      && !packagedProofRun.includes("--proof-tier installed_runtime"),
    `${file} optional hosted CPU lane must remain evaluation-only`,
  );
  add(
    violations,
    !scalarStrings(workflow).some(value =>
      value.includes("candidate_installed")
      || value.includes("candidate-installed")
      || value.includes("managed plugin handoff")
      || value.includes("scope == 'server'")
    ),
    `${file} package-only workflow must not contain installed-runtime or server-scope routing`,
  );
  requireStepUses(violations, file, job, "Upload release asset", "actions/upload-artifact@v7.0.1");
  requireStepUses(violations, file, job, "Upload macOS notarization proof", "actions/upload-artifact@v7.0.1");
  requireStepRun(violations, file, job, "Emit authenticated package release cell", [
    "codestory-release-cell-manifest.mjs produce",
    "package_identity:${{ matrix.asset_target }}",
    "--producer-job build",
    "--archive",
  ]);
  const packageCellUpload = namedStep(job, "Upload authenticated package release cell");
  add(
    violations,
    packageCellUpload?.uses === "actions/upload-artifact@v7.0.1"
      && String(packageCellUpload?.if ?? "").includes("success()")
      && String(packageCellUpload?.if ?? "").includes("inputs.emit_release_cells"),
    `${file} package release cell must be a success-only retained artifact`,
  );
}

function validatePostPublish(workflows, violations) {
  const file = "post-publish-release-smoke.yml";
  const workflow = workflows.get(file);
  if (!workflow) {
    violations.push(`${file} must exist`);
    return;
  }
  add(violations, trigger(workflow, "workflow_call") !== undefined, `${file} must be reusable`);
  add(violations, object(workflow.permissions).actions === "read", `${file} must read the accepted pre-publish closeout`);
  requireNoCalibrationReferences(violations, file, workflow);
  for (const event of ["workflow_call", "workflow_dispatch"]) {
    const marketplaceInput = object(
      at(workflow, "on", event, "inputs", "marketplace_revision"),
    );
    add(
      violations,
      marketplaceInput.required === true && marketplaceInput.type === "string",
      `${file} ${event} marketplace_revision must be a required string`,
    );
    const closeoutInput = object(at(workflow, "on", event, "inputs", "pre_publish_closeout_artifact"));
    add(violations, closeoutInput.type === "string", `${file} ${event} pre_publish_closeout_artifact must be a string`);
  }
  const job = requireJob(violations, file, workflow, "smoke");
  const pythonSetup = namedStep(job, "Install pinned Python");
  add(
    violations,
    pythonSetup?.uses === "actions/setup-python@v7.0.0"
      && pythonSetup?.if === "runner.os != 'macOS'"
      && object(pythonSetup?.with)["python-version"] === "3.13"
      && object(pythonSetup?.env).PSExecutionPolicyPreference === "Bypass",
    `${file} must install pinned Python with the protected Windows execution policy`,
  );
  const macosPythonSetup = namedStep(job, "Install pinned Python on macOS");
  add(
    violations,
    macosPythonSetup?.if === "runner.os == 'macOS'"
      && macosPythonSetup?.shell === "bash",
    `${file} must install pinned Python through the protected macOS user toolchain`,
  );
  requireStepRun(violations, file, job, "Install pinned Python on macOS", [
    "uv python install 3.13.14",
    'python_bin="$(uv python find 3.13.14)"',
    "platform.python_version()",
    'echo "$shim_dir" >> "$GITHUB_PATH"',
  ]);
  const expected = expectedPostPublishRows();
  add(
    violations,
    JSON.stringify(at(job, "strategy", "matrix", "include")) === JSON.stringify(expected),
    `${file} must run the three supported release assets on protected accelerated hosts`,
  );
  add(
    violations,
    job["runs-on"] === "${{ fromJSON(matrix.runs_on) }}"
      && job.environment === "${{ matrix.environment }}",
    `${file} smoke job must bind each asset to its protected runner and environment`,
  );
  add(
    violations,
    at(job, "strategy", "fail-fast") === false,
    `${file} protected matrix must not cancel sibling platform proof`,
  );
  add(
    violations,
    object(workflow.env).CODEX_CLI_VERSION === "0.144.5",
    `${file} must pin the Codex CLI used for marketplace installation`,
  );
  const resolveInstalled = namedStep(job, "Resolve the published plugin through the marketplace catalog");
  requireStepRun(violations, file, job, "Resolve the published plugin through the marketplace catalog", [
    'marketplace_revision="${{ inputs.marketplace_revision }}"',
    '"@openai/codex@$CODEX_CLI_VERSION"',
    "install-codestory-marketplace-proof.mjs",
    "TheGreenCedar/AgentPluginMarketplace",
    '--marketplace-revision "$marketplace_revision"',
    '--source-repository "$GITHUB_WORKSPACE"',
    "install-attestation-v2.json",
    'isolated_home="$install_root/isolated-home"',
    'HOME="$isolated_home" node',
  ]);
  add(
    violations,
    namedStep(job, "Prove packaged version, help, and stdio shape")?.shell === "bash",
    `${file} packaged Python proof must use Bash on every protected platform`,
  );
  const resolveRun = executableRunText(String(resolveInstalled?.run ?? ""));
  for (const forbidden of [
    "git archive",
    "git clone",
    "git ls-remote",
    "plugin_package_sha256",
    "--source-commit",
    "--source-tree",
  ]) {
    add(
      violations,
      !resolveRun.includes(forbidden),
      `${file} marketplace install must not fabricate installation with ${forbidden}`,
    );
  }
  add(
    violations,
    resolveInstalled?.if === undefined
      && resolveInstalled?.["continue-on-error"] === undefined,
    `${file} installed plugin resolution must be unconditional and fail closed`,
  );
  const installed = namedStep(job, "Prove the catalog-resolved published runtime");
  add(violations, installed !== undefined, `${file} installed runtime proof step is missing`);
  add(
    violations,
    installed?.if === undefined
      && installed?.["continue-on-error"] === undefined,
    `${file} installed runtime proof must be unconditional and fail closed`,
  );
  add(
    violations,
    object(installed?.env).CODESTORY_EMBED_ALLOW_CPU === "0",
    `${file} installed runtime proof must reject CPU fallback`,
  );
  const installedRun = executableRunText(String(installed?.run ?? ""));
  for (const fragment of [
    "python .github/scripts/check-packaged-agent-proof.py",
    '--archive "${{ steps.asset.outputs.archive }}"',
    "--plugin-handoff",
    "--engine-policy accelerated",
    '--expected-backend "${{ matrix.backend }}"',
    "--proof-tier installed_runtime",
    "--server-behavior-only",
    "--installed-plugin-attestation",
    "--installed-plugin-data",
    "--expected-source-sha",
    "--expected-source-tree",
  ]) {
    add(
      violations,
      installedRun.includes(fragment),
      `${file} installed runtime proof must run ${fragment}`,
    );
  }
  for (const fragment of ["--engine-policy cpu_explicit", "--expected-backend CPU", "--ground-only"]) {
    add(
      violations,
      !installedRun.includes(fragment),
      `${file} installed runtime proof must not run ${fragment}`,
    );
  }
  requireStepUses(
    violations,
    file,
    job,
    "Download accepted pre-publish closeout",
    "actions/download-artifact@v8.0.1",
  );
  requireStepRun(violations, file, job, "Emit authenticated post-publish release cells", [
    "platform_support:${{ matrix.asset_target }}",
    "installed_runtime_behavior:${{ matrix.asset_target }}",
    "post_publish_bytes:${{ matrix.asset_target }}",
    "--pre-publish-ledger",
    "release-cell-postpublish-${{ matrix.asset_target }}",
  ]);
  requireStepUses(
    violations,
    file,
    job,
    "Upload authenticated post-publish release cells",
    "actions/upload-artifact@v7.0.1",
  );
  const postCellUpload = namedStep(job, "Upload authenticated post-publish release cells");
  add(
    violations,
    String(postCellUpload?.if ?? "").includes("success()")
      && String(postCellUpload?.if ?? "").includes("inputs.emit_release_cells"),
    `${file} post-publish release cells must be success-only artifacts`,
  );
  add(
    violations,
    namedStep(job, "Install pinned Rust for Windows rollback proof") === undefined
      && object(workflow.env).RELEASE_RUST_TOOLCHAIN === undefined,
    `${file} published ground proof must not install unused Rust tooling`,
  );
  add(
    violations,
    !installedRun.includes("--offline"),
    `${file} installed runtime proof must allow the managed launcher to provision the release asset`,
  );
  const macProof = namedStep(job, "Prove published macOS signature, notarization, and quarantined execution");
  requireStepRun(violations, file, job, "Prove published macOS signature, notarization, and quarantined execution", [
    "archive-quarantine.txt",
    "extracted-binary-quarantine.txt",
    "Authority=Developer ID Application:",
    "TeamIdentifier=${APPLE_DEVELOPER_TEAM_ID}",
    "certificate leaf",
  ]);
  violations.push(...macosCliDistributionViolations(macProof, macProof, '"$bin"').map(message => `${file} ${message}`));
  const windowsInstaller = namedStep(job, "Run Windows installer ownership self-test");
  add(
    violations,
    windowsInstaller?.shell
      === `powershell -NoLogo -NoProfile -NonInteractive -ExecutionPolicy Bypass -Command ". '{0}'"`,
    `${file} Windows installer proof must bypass host execution policy explicitly`,
  );
  requireStepRun(violations, file, job, "Run Windows installer ownership self-test", ["scripts/install-codestory.ps1 -SelfTest"]);
  add(violations, !scalarStrings(workflow).some(value => value.includes("sha256sum")), `${file} must use the portable Python checksum gate`);
}

function validatePackagedCoordinator(workflows, violations, graph) {
  const file = "packaged-platform-pr.yml";
  const workflow = workflows.get(file);
  if (!workflow) {
    violations.push(`${file} must exist`);
    return;
  }
  const promotion = graph.workflow_policy.promotion;
  const expectedConcurrency = [
    "proof-",
    promotion.proof_run_sha_expression,
    "-${{ inputs.mode || 'platform' }}-${{ inputs.pr_number || github.event.pull_request.number || 'dev' }}-",
    "${{ github.event.action == 'labeled' && github.event.label.name || 'dispatch' }}",
  ].join("");
  add(
    violations,
    sameMembers(at(workflow, "on", "pull_request", "types"), promotion.required_events),
    `${file} pull request trigger must be label-only`,
  );
  add(
    violations,
    at(workflow, "concurrency", "group") === expectedConcurrency,
    `${file} concurrency must bind the Actions SHA, mode, PR identity, and exact label`,
  );
  add(
    violations,
    String(at(workflow, "on", "workflow_dispatch", "inputs", "pr_number", "description") ?? "")
      .includes(promotion.manual_pr_ref_hint),
    `${file} manual PR input must require ${promotion.manual_pr_ref_hint}`,
  );
  add(
    violations,
    sameMembers(
      at(workflow, "on", "workflow_dispatch", "inputs", "mode", "options"),
      ["package", "platform", "qualification", "calibration", "release-evidence", "integration"],
    ),
    `${file} dispatch modes changed`,
  );
  add(
    violations,
    sameMembers(
      at(workflow, "on", "workflow_dispatch", "inputs", "scope", "options"),
      ["auto", "none", "linux", "windows", "macos", "full"],
    ),
    `${file} dispatch scopes changed`,
  );
  add(violations, trigger(workflow, "pull_request_target") === undefined, `${file} must not use pull_request_target`);
  add(violations, object(workflow.permissions).actions === "read", `${file} must read source-proof runs`);
  add(violations, object(workflow.permissions).contents === "read", `${file} must use read-only contents permission`);
  const route = requireJob(violations, file, workflow, "route");
  add(
    violations,
    route.if === "github.event_name != 'pull_request' || (github.event.action == 'labeled' && github.event.label.name == 'platform-proof')",
    `${file} route job must execute dispatch runs and only platform-proof labeled PR runs`,
  );
  requireStepRun(violations, file, route, "Resolve trusted exact head", [
    'test "$head_repo" = "$GITHUB_REPOSITORY"',
    'test "$current_head" = "$EVENT_HEAD_SHA"',
    'test "$INPUT_HEAD_SHA" = "$current_head"',
    'test "$GITHUB_REF" = "refs/heads/$head_ref"',
    'test "$GITHUB_SHA" = "$INPUT_HEAD_SHA"',
    'test "$base_ref" = "dev/codestory-next"',
    'test "$INPUT_HEAD_SHA" = "$dev_head"',
    'test "$GITHUB_REF" = "refs/heads/dev/codestory-next"',
    'test "$GITHUB_SHA" = "$dev_head"',
    "--ref $head_ref",
  ]);
  requireExactResolverContract(violations, file, route, platformResolverContractDigest);
  requireStepRun(violations, file, route, "Require successful exact-head source proof", [
    "actions/runs?head_sha=$HEAD_SHA",
    '.path == ".github/workflows/source-proof.yml"',
    '.name == "full-source-gate" and .conclusion == "success"',
  ]);
  requireStepRun(violations, file, route, "Select change-aware proof scope", [
    'if [ "$REQUESTED_SCOPE" = none ] || [ "$REQUESTED_SCOPE" = linux ]; then',
    'elif [ "${{ steps.resolve.outputs.mode }}" = "package" ]; then',
    'test "$REQUESTED_SCOPE" != none',
    'if [ "$REQUESTED_SCOPE" = auto ]; then',
    'elif [ "${{ steps.resolve.outputs.mode }}" = "qualification" ]; then',
    'test "$REQUESTED_SCOPE" = auto || test "$REQUESTED_SCOPE" = full',
    'scope="$REQUESTED_SCOPE"',
    "scope=full",
    "node .github/scripts/route-ci-proof.mjs --stdin",
  ]);
  add(
    violations,
    String(namedStep(route, "Select change-aware proof scope")?.run ?? "")
      .includes('if [ "$REQUESTED_SCOPE" = none ] || [ "$REQUESTED_SCOPE" = linux ]; then'),
    `${file} integration must preserve explicit no-op and Linux scopes`,
  );
  add(
    violations,
    namedStep(route, "Read qualification constant-set state") === undefined
      && at(route, "outputs", "constants_frozen") === undefined
      && at(route, "outputs", "freeze_transition") === undefined
      && !scalarStrings(workflow).some(value =>
        value.includes("enforce_calibration_freeze_lineage")
        || value.includes("freeze_transition")
        || value.includes("base_frozen")
      ),
    `${file} standard coordinator must not gate release proof on constant-set freeze state`,
  );
  requireCalibrationProducerAuthentication(violations, file, route);
  const routeSteps = list(route.steps).map(object);
  add(
    violations,
    routeSteps.findIndex(step => step.name === "Resolve trusted exact head") === 0
      && routeSteps.findIndex(step => step.uses === "actions/checkout@v5") > 0,
    `${file} must resolve exact workflow/ref identity before checkout`,
  );
  const calibrationLinux = requireJob(violations, file, workflow, "calibration-linux");
  add(
    violations,
    calibrationLinux.uses === "./.github/workflows/packaged-platform-proof.yml"
      && object(calibrationLinux.with).calibration_mode === true
      && object(calibrationLinux.with).hermetic_linux === undefined,
    `${file} hosted Linux calibration must call packaged proof in calibration mode`,
  );
  const calibrationMacos = requireJob(violations, file, workflow, "calibration-macos");
  add(
    violations,
    calibrationMacos.uses === "./.github/workflows/macos-metal-proof.yml"
      && object(calibrationMacos.with).calibration_mode === true,
    `${file} protected macOS calibration must call Metal proof in calibration mode`,
  );
  add(
    violations,
    at(workflow, "jobs", "macos-source") === undefined
      && at(workflow, "jobs", "repo-scale-stats") === undefined,
    `${file} standard coordinator must not add macOS source or repo-scale hard gates`,
  );
  const calibrationAssemble = requireJob(
    violations,
    file,
    workflow,
    "calibration-assemble",
  );
  add(
    violations,
    sameMembers(needs(calibrationAssemble), [
      "route",
      "calibration-linux",
      "calibration-macos",
    ]),
    `${file} calibration assembly must wait for both independent calibration cells`,
  );
  requireStepRun(
    violations,
    file,
    calibrationAssemble,
    "Assemble frozen calibration candidate",
    [
      "--assemble-calibration-bundle",
      'test "${#runs[@]}" = 6',
      "--calibration-producer-workflow-path",
      "--calibration-producer-run-id",
      "--calibration-producer-artifact",
    ],
  );
  requireStepUses(
    violations,
    file,
    calibrationAssemble,
    "Upload calibration bundle and frozen constant candidate",
    "actions/upload-artifact@v7.0.1",
  );
  add(
    violations,
    object(namedStep(
      calibrationAssemble,
      "Upload calibration bundle and frozen constant candidate",
    )?.with).name
      === "embedding-calibration-bundle-${{ needs.route.outputs.head_sha }}",
    `${file} calibration artifact name must bind the exact source head`,
  );
  const packaged = requireJob(violations, file, workflow, "packaged-proof");
  add(violations, packaged.uses === "./.github/workflows/packaged-platform-proof.yml", `${file} must call packaged proof`);
  add(
    violations,
    String(packaged.if ?? "").includes("needs.route.outputs.mode == 'package'")
      && String(packaged.if ?? "").includes("needs.route.outputs.mode == 'platform'")
      && String(packaged.if ?? "").includes("needs.route.outputs.mode == 'qualification'")
      && object(packaged.with).hermetic_linux
        === "${{ needs.route.outputs.mode == 'qualification' }}",
    `${file} package and platform modes must build fresh archives while only qualification runs the cold Linux boundary`,
  );
  for (const key of [
    "candidate_installed_proof",
    "candidate_installed_only",
    "server_behavior_only",
    "enforce_calibration_freeze_lineage",
  ]) {
    add(
      violations,
      object(packaged.with)[key] === undefined,
      `${file} package-only call must not pass ${key}`,
    );
  }
  add(
    violations,
    !String(packaged.if ?? "").includes("release-evidence")
      && !needs(packaged).includes("release-evidence")
      && object(packaged.with).quality_evidence_artifact === "",
    `${file} package proof must not depend on optional release evidence`,
  );
  violations.push(...packagedPrSigningViolations(workflow));
  const metal = requireJob(violations, file, workflow, "macos-metal-proof");
  add(
    violations,
    sameMembers(needs(metal), ["route", "packaged-proof"]),
    `${file} Metal proof must wait only for routing and package proof`,
  );
  add(
    violations,
    String(metal.if ?? "").includes("needs.route.outputs.mode != 'package'"),
    `${file} package-only mode must skip protected Metal proof`,
  );
  add(violations, object(metal.with).use_packaged_cli_artifact === true, `${file} Metal proof must use the packaged CLI`);
  add(
    violations,
    object(metal.with).candidate_installed_proof === true,
    `${file} must opt the accepted PR Metal package into candidate-installed proof`,
  );
  add(
    violations,
    object(metal.with).server_behavior_only === true
      && object(metal.with).quality_evidence_artifact === "",
    `${file} Metal proof must use bounded readiness without optional quality evidence`,
  );
  const vulkan = requireJob(violations, file, workflow, "windows-vulkan-proof");
  add(
    violations,
    sameMembers(needs(vulkan), ["route", "packaged-proof"]),
    `${file} Vulkan proof must wait only for routing and package proof`,
  );
  add(
    violations,
    String(vulkan.if ?? "").includes("needs.route.outputs.mode != 'package'"),
    `${file} package-only mode must skip protected Windows proof`,
  );
  add(violations, object(vulkan.with).use_packaged_cli_artifact === true, `${file} Vulkan proof must use the packaged CLI`);
  add(
    violations,
    object(vulkan.with).candidate_installed_proof === true,
    `${file} must opt the accepted PR Windows package into candidate-installed proof`,
  );
  add(
    violations,
    object(vulkan.with).quality_evidence_artifact === "",
    `${file} Windows proof must not consume optional quality evidence`,
  );
  add(
    violations,
    object(vulkan.with).server_behavior_only === true,
    `${file} Windows proof must use bounded retrieval readiness`,
  );
  const linuxVulkan = requireJob(violations, file, workflow, "linux-vulkan-proof");
  add(
    violations,
    sameMembers(needs(linuxVulkan), ["route", "packaged-proof"]),
    `${file} Linux Vulkan proof must wait only for routing and package proof`,
  );
  add(
    violations,
    String(linuxVulkan.if ?? "").includes("needs.route.outputs.mode != 'package'"),
    `${file} package-only mode must skip protected Linux proof`,
  );
  add(
    violations,
    linuxVulkan.uses === "./.github/workflows/linux-vulkan-proof.yml",
    `${file} Linux proof must use the protected Vulkan workflow`,
  );
  add(
    violations,
    object(linuxVulkan.with).candidate_installed_proof === true
      && object(linuxVulkan.with).server_behavior_only === true
      && object(linuxVulkan.with).candidate_producer_workflow_path
        === ".github/workflows/packaged-platform-pr.yml",
    `${file} Linux proof must close Vulkan and candidate-installed claims without optional evaluation`,
  );
  const closeout = requireJob(violations, file, workflow, "closeout");
  add(
    violations,
    sameMembers(needs(closeout), [
      "route",
      "source-proof",
      "packaged-proof",
      "macos-metal-proof",
      "windows-vulkan-proof",
      "linux-vulkan-proof",
    ]),
    `${file} closeout must wait for every selected platform proof`,
  );
  const evidence = requireJob(violations, file, workflow, "release-evidence");
  add(
    violations,
    evidence.if === "needs.route.outputs.mode == 'release-evidence'",
    `${file} optional release evidence must run only in explicit release-evidence mode`,
  );
  add(
    violations,
    !needs(closeout).includes("release-evidence")
      && !scalarStrings(closeout).some(value => value.includes("EVIDENCE_RESULT")),
    `${file} normal closeout must not depend on optional release evidence`,
  );
  requireStepRun(violations, file, closeout, "Require one coherent accepted proof", [
    'if [ "$MODE" = package ]',
    'require_result "$PACKAGE_RESULT" success packaged-proof',
    'require_result "$METAL_RESULT" skipped macos-metal-proof',
    'if [ "$SCOPE" = none ]',
    '[ "$SCOPE" = linux ]',
    "WINDOWS_VULKAN_RESULT",
    "LINUX_VULKAN_RESULT",
    'require_result "$LINUX_VULKAN_RESULT" success linux-vulkan-proof',
    "dev/codestory-next moved from proved head",
  ]);
  add(violations, !scalarStrings(workflow).some(value => value === "./.github/workflows/release.yml"), `${file} must not publish releases`);
}

function validateRemainingWorkflows(workflows, violations) {
  const autoFile = "auto-release.yml";
  const auto = workflows.get(autoFile);
  if (!auto) {
    violations.push(`${autoFile} must exist`);
  } else {
    requireNoCalibrationReferences(violations, autoFile, auto);
    add(violations, includesAll(at(auto, "on", "push", "branches"), ["main"]), `${autoFile} must run on main`);
    add(violations, includesAll(at(auto, "on", "push", "paths"), [
      "package.json",
      "package-lock.json",
      "release-claims.json",
      ".github/actionlint.yaml",
      ".github/workflows/**",
      "scripts/codestory-release-*.mjs",
      "scripts/tests/codestory-release-*.test.mjs",
    ]), `${autoFile} must observe policy dependency and release-claim changes`);
    add(
      violations,
      object(auto.jobs)["workflow-policy"] === undefined,
      `${autoFile} must delegate the release policy gate to release.yml exactly once`,
    );
    const detectVersion = requireJob(violations, autoFile, auto, "detect-version");
    add(
      violations,
      needs(detectVersion).length === 0,
      `${autoFile} version detection must not depend on a duplicate policy gate`,
    );
    add(
      violations,
      namedStep(detectVersion, "Validate synchronized release version") === undefined,
      `${autoFile} must delegate synchronized version validation to release.yml`,
    );
    const release = requireJob(violations, autoFile, auto, "release");
    add(violations, release.uses === "./.github/workflows/release.yml", `${autoFile} must call the release workflow`);
    add(violations, sameMembers(needs(release), ["detect-version"]), `${autoFile} release must need version detection`);
    add(violations, object(release.permissions).contents === "write", `${autoFile} release caller must grant contents write`);
    add(violations, object(release.permissions).actions === "read", `${autoFile} release caller must grant actions read`);
    add(
      violations,
      object(release.permissions)["pull-requests"] === "read",
      `${autoFile} release caller must pass pull-request metadata access to exact source proof`,
    );
    add(violations, object(release.with).publish_release === true, `${autoFile} trusted main caller must explicitly authorize publication`);
    add(
      violations,
      release.secrets === "inherit",
      `${autoFile} release caller must inherit repository release secrets`,
    );
  }

  const evidenceFile = "release-candidate-evidence.yml";
  const evidence = workflows.get(evidenceFile);
  if (!evidence) {
    violations.push(`${evidenceFile} must exist`);
  } else {
    add(violations, trigger(evidence, "workflow_call") !== undefined, `${evidenceFile} must be reusable`);
    add(violations, trigger(evidence, "workflow_dispatch") === undefined, `${evidenceFile} must be coordinator-only`);
    const job = requireJob(violations, evidenceFile, evidence, "measure");
    add(violations, JSON.stringify(job["runs-on"]) === JSON.stringify(["self-hosted", "Linux", "ARM64", "codestory-release-evidence"]), `${evidenceFile} must use the protected evidence runner`);
    requireStepRun(violations, evidenceFile, job, "Prepare checksum-pinned embedded model", ["node scripts/prepare-embedded-model.mjs"]);
    violations.push(...releaseEvidenceApprovalViolations(
      [
        ["packaged-platform-pr.yml", at(workflows.get("packaged-platform-pr.yml"), "jobs", "release-evidence"), false],
      ],
      evidence,
    ));
    const repoEvidence = namedStep(job, "Produce full-retrieval repo evidence");
    requireStepRun(violations, evidenceFile, job, "Produce full-retrieval repo evidence", ["--test-threads=1"]);
    add(
      violations,
      object(repoEvidence?.env).CODESTORY_RELEASE_EVIDENCE_CORPUS_ID
        === "codestory-release-corpus-v0.16-axios-js-ts-v2",
      `${evidenceFile} repo evidence must bind the v0.16 Axios v2 corpus`,
    );
    const packetEvidence = namedStep(job, "Produce publishable packet evidence");
    add(
      violations,
      object(packetEvidence?.env).CODESTORY_RELEASE_EVIDENCE_CORPUS_ID
        === "codestory-release-corpus-v0.16-axios-js-ts-v2"
        && object(packetEvidence?.env).CODESTORY_RELEASE_EVIDENCE_CORPUS_CONTRACT
          === "benchmarks/release-evidence/corpus-contracts/v0.16-axios-js-ts-v2.json",
      `${evidenceFile} packet evidence must bind the v0.16 Axios v2 corpus contract`,
    );
    const packetRun = String(packetEvidence?.run ?? "");
    add(
      violations,
      packetRun.includes("--task-manifest ")
        && packetRun.match(/--task-manifest/gu)?.length === 1
        && !packetRun.includes("--task-suite")
        && !packetRun.includes("--task-ids"),
      `${evidenceFile} packet evidence must select only the corpus-bound release task manifest`,
    );
    requireStepRun(violations, evidenceFile, job, "Download prior rejected evidence for approval re-evaluation", ["actions/runs/$SOURCE_RUN_ID", "actions/runs/$SOURCE_RUN_ID/artifacts"]);
  }

  const metalFile = "macos-metal-proof.yml";
  const metal = workflows.get(metalFile);
  if (!metal) {
    violations.push(`${metalFile} must exist`);
  } else {
    add(violations, trigger(metal, "workflow_call") !== undefined && trigger(metal, "workflow_dispatch") !== undefined, `${metalFile} must support reusable and manual proof`);
    for (const event of ["workflow_call", "workflow_dispatch"]) {
      for (const key of ["calibration_bundle_artifact", "calibration_bundle_run_id"]) {
        requireOptionalStringInput(violations, metalFile, metal, event, key);
      }
    }
    const candidateInput = object(at(
      metal,
      "on",
      "workflow_call",
      "inputs",
      "candidate_installed_proof",
    ));
    add(
      violations,
      candidateInput.required === false
        && candidateInput.type === "boolean"
        && candidateInput.default === false,
      `${metalFile} candidate-installed proof must be an explicit opt-in`,
    );
    for (const event of ["workflow_call", "workflow_dispatch"]) {
      add(
        violations,
        at(metal, "on", event, "inputs", "candidate_installed_only") === undefined,
        `${metalFile} ${event} must not define candidate_installed_only`,
      );
    }
    const candidateProducerInput = object(at(
      metal,
      "on",
      "workflow_call",
      "inputs",
      "candidate_producer_workflow_path",
    ));
    add(
      violations,
      candidateProducerInput.required === false
        && candidateProducerInput.type === "string"
        && candidateProducerInput.default === ".github/workflows/packaged-platform-pr.yml",
      `${metalFile} candidate producer path must default to the exact PR coordinator`,
    );
    const serverBehaviorInput = object(at(
      metal,
      "on",
      "workflow_call",
      "inputs",
      "server_behavior_only",
    ));
    add(
      violations,
      serverBehaviorInput.required === false
        && serverBehaviorInput.type === "boolean"
        && serverBehaviorInput.default === false,
      `${metalFile} server-behavior-only claim scope must be an explicit opt-in`,
    );
    const job = requireJob(violations, metalFile, metal, "packaged-metal");
    add(violations, JSON.stringify(job["runs-on"]) === JSON.stringify(["self-hosted", "macOS", "ARM64", "codestory-metal"]), `${metalFile} must use the protected Apple Silicon runner`);
    add(violations, job.environment === "macos-metal-release", `${metalFile} must use the protected Metal environment`);
    const validateCandidate = namedStep(job, "Validate candidate-installed mode");
    add(
      violations,
      validateCandidate?.if === "inputs.candidate_installed_proof"
        && validateCandidate?.shell === "bash",
      `${metalFile} candidate-installed validation must be an explicit Bash boundary`,
    );
    requireStepRun(violations, metalFile, job, "Validate candidate-installed mode", [
      'test "${{ inputs.server_behavior_only }}" = true',
      'test "${{ inputs.calibration_mode }}" = false',
    ]);
    requireStepRun(violations, metalFile, job, "Prepare checksum-pinned embedded model", ["node scripts/prepare-embedded-model.mjs"]);
    requireStepRun(violations, metalFile, job, "Capture host evidence", ["python3 --version", 'test "$macos_major" -ge 15']);
    add(
      violations,
      namedStep(job, "Install pinned Rust")?.if
        === "${{ !inputs.use_packaged_cli_artifact || inputs.calibration_mode || !inputs.server_behavior_only }}",
      `${metalFile} packaged server-behavior proof must skip unused Rust installation`,
    );
    add(
      violations,
      namedStep(job, "Build qualification driver")?.if
        === "inputs.calibration_mode || !inputs.server_behavior_only",
      `${metalFile} packaged server-behavior proof must skip the qualification driver`,
    );
    const packagedArtifactDownload = namedStep(job, "Download packaged CLI artifact");
    add(
      violations,
      packagedArtifactDownload?.if === "inputs.use_packaged_cli_artifact"
        && packagedArtifactDownload?.shell === "bash"
        && object(packagedArtifactDownload?.env).GH_TOKEN === "${{ github.token }}"
        && object(packagedArtifactDownload?.env).ARTIFACT_NAME === "codestory-cli-macos-arm64",
      `${metalFile} packaged CLI download must be an authenticated exact-artifact Bash boundary`,
    );
    requireStepRun(violations, metalFile, job, "Download packaged CLI artifact", [
      "actions/runs/$GITHUB_RUN_ID/artifacts?per_page=100",
      ".workflow_run.id == $run_id",
      ".workflow_run.head_sha == $sha",
      ".digest",
      ".size_in_bytes",
      "--continue-at -",
      "--max-time 120",
      "test \"$actual_size\" = \"$expected_size\"",
      "test \"$actual_digest\" = \"${expected_digest#sha256:}\"",
      "ditto -x -k",
    ]);
    requireCalibrationProducerBoundary(
      violations,
      metalFile,
      job,
      "${{ !inputs.calibration_mode && !inputs.server_behavior_only }}",
    );
    const engine = namedStep(job, "Prove protected Metal runtime");
    requireStepRun(violations, metalFile, job, "Prove protected Metal runtime", [
      "--engine-policy accelerated",
      "--expected-backend Metal",
      "--offline",
      "--proof-tier protected_hardware",
      "--qualification-matrix-cell protected_macos_arm64_metal",
      "--calibration-producer-run-id",
      "--calibration-producer-artifact",
      "--server-behavior-only",
      'test -f "$quality_path"',
    ]);
    add(violations, object(engine?.env).CODESTORY_EMBED_ALLOW_CPU === "0", `${metalFile} engine proof must reject CPU fallback`);
    const engineRun = stepRun(
      job,
      "Prove protected Metal runtime",
    );
    add(
      violations,
      engineRun.includes("calibration_args=()")
        && engineRun.includes('"${calibration_args[@]}"')
        && engineRun.includes('claim_scope_args=(--server-behavior-only)')
        && occurrenceCount(engineRun, "--calibration-bundle") === 1,
      `${metalFile} server-behavior proof must omit calibration while qualification retains it`,
    );
    add(
      violations,
      engine?.if === "${{ !inputs.calibration_mode && !inputs.candidate_installed_proof }}",
      `${metalFile} protected Metal proof must yield to the candidate-installed lane`,
    );
    const candidateStage = namedStep(job, "Stage isolated candidate-managed macOS install");
    add(
      violations,
      candidateStage?.if === "${{ inputs.candidate_installed_proof && !inputs.calibration_mode }}",
      `${metalFile} candidate-managed staging must require candidate mode outside calibration`,
    );
    requireStepRun(violations, metalFile, job, "Stage isolated candidate-managed macOS install", [
      "--prepare-candidate-installed-proof",
      "--candidate-plugin-root-output",
      "--candidate-plugin-data-output",
      "--installed-plugin-attestation-output",
      "--candidate-producer-workflow-path",
      "gh api",
      ".head_repository.full_name",
      ".path",
      ".head_sha",
      ".run_attempt",
      "$CANDIDATE_PRODUCER_WORKFLOW_PATH",
      "$RUNNER_TEMP/codestory-candidate-installed-macos.",
      'candidate_root="$(cd "$candidate_root" && pwd -P)"',
      '"$GITHUB_WORKSPACE/"*',
      "CODESTORY_CANDIDATE_MACOS_ROOT=",
    ]);
    const candidateProof = namedStep(job, "Prove candidate-installed macOS Metal runtime");
    add(
      violations,
      candidateProof?.if === "${{ inputs.candidate_installed_proof && !inputs.calibration_mode }}",
      `${metalFile} candidate-installed Metal proof must require candidate mode outside calibration`,
    );
    requireStepRun(violations, metalFile, job, "Prove candidate-installed macOS Metal runtime", [
      "--engine-policy accelerated",
      "--expected-backend Metal",
      "--proof-tier installed_runtime",
      "--installed-plugin-attestation",
      "--installed-plugin-data",
      "$CANDIDATE_PRODUCER_WORKFLOW_PATH",
      "--server-behavior-only",
      "$CODESTORY_CANDIDATE_MACOS_ROOT/plugin",
      "$CODESTORY_CANDIDATE_MACOS_ROOT/data",
    ]);
    const candidateProofRun = stepRun(
      job,
      "Prove candidate-installed macOS Metal runtime",
    );
    add(
      violations,
      !candidateProofRun.includes("calibration")
        && !candidateProofRun.includes("--ground-only")
        && !candidateProofRun.includes("--engine-policy cpu_explicit"),
      `${metalFile} candidate-installed Metal proof must be bounded accelerated runtime proof`,
    );
    add(
      violations,
      object(candidateProof?.env).CODESTORY_EMBED_ALLOW_CPU === "0",
      `${metalFile} candidate-installed proof must reject CPU fallback`,
    );
    requireStepRun(violations, metalFile, job, "Emit authenticated Metal release cell", [
      "codestory-release-cell-manifest.mjs produce",
      "accelerator_execution:macos-arm64-metal",
      "--producer-job packaged-metal",
    ]);
    const metalCellUpload = namedStep(job, "Upload authenticated Metal release cell");
    add(
      violations,
      metalCellUpload?.uses === "actions/upload-artifact@v7.0.1"
        && String(metalCellUpload?.if ?? "").includes("success()")
        && String(metalCellUpload?.if ?? "").includes("inputs.emit_release_cells"),
      `${metalFile} Metal release cell must be a success-only retained artifact`,
    );
    requireStepRun(violations, metalFile, job, "Emit authenticated candidate-installed macOS release cell", [
      "candidate_installed_behavior:macos-arm64",
      "--producer-job packaged-metal",
      "candidate_managed_plugin",
    ]);
    forbidStepRun(
      violations,
      metalFile,
      job,
      "Emit authenticated Metal release cell",
      ["calibration"],
    );
    forbidStepRun(
      violations,
      metalFile,
      job,
      "Emit authenticated candidate-installed macOS release cell",
      ["calibration"],
    );
    requireStepUses(
      violations,
      metalFile,
      job,
      "Upload authenticated candidate-installed macOS release cell",
      "actions/upload-artifact@v7.0.1",
    );
  }

  const vulkanFile = "windows-vulkan-proof.yml";
  const vulkan = workflows.get(vulkanFile);
  if (!vulkan) {
    violations.push(`${vulkanFile} must exist`);
  } else {
    add(violations, trigger(vulkan, "workflow_call") !== undefined && trigger(vulkan, "workflow_dispatch") !== undefined, `${vulkanFile} must support reusable and manual proof`);
    for (const event of ["workflow_call", "workflow_dispatch"]) {
      for (const key of ["calibration_bundle_artifact", "calibration_bundle_run_id"]) {
        requireOptionalStringInput(violations, vulkanFile, vulkan, event, key);
      }
    }
    const candidateInput = object(at(
      vulkan,
      "on",
      "workflow_call",
      "inputs",
      "candidate_installed_proof",
    ));
    add(
      violations,
      candidateInput.required === false
        && candidateInput.type === "boolean"
        && candidateInput.default === false,
      `${vulkanFile} candidate-installed proof must be an explicit opt-in`,
    );
    for (const event of ["workflow_call", "workflow_dispatch"]) {
      add(
        violations,
        at(vulkan, "on", event, "inputs", "candidate_installed_only") === undefined,
        `${vulkanFile} ${event} must not define candidate_installed_only`,
      );
    }
    const candidateProducerInput = object(at(
      vulkan,
      "on",
      "workflow_call",
      "inputs",
      "candidate_producer_workflow_path",
    ));
    add(
      violations,
      candidateProducerInput.required === false
        && candidateProducerInput.type === "string"
        && candidateProducerInput.default === ".github/workflows/packaged-platform-pr.yml",
      `${vulkanFile} candidate producer path must default to the exact PR coordinator`,
    );
    const serverBehaviorInput = object(at(
      vulkan,
      "on",
      "workflow_call",
      "inputs",
      "server_behavior_only",
    ));
    add(
      violations,
      serverBehaviorInput.required === false
        && serverBehaviorInput.type === "boolean"
        && serverBehaviorInput.default === false,
      `${vulkanFile} server-behavior-only claim scope must be an explicit opt-in`,
    );
    const job = requireJob(violations, vulkanFile, vulkan, "packaged-vulkan");
    add(violations, JSON.stringify(job["runs-on"]) === JSON.stringify(["self-hosted", "Windows", "X64", "codestory-vulkan"]), `${vulkanFile} must use the protected Windows Vulkan runner`);
    add(violations, job.environment === "windows-vulkan-proof", `${vulkanFile} must use the protected Vulkan environment`);
    const pythonSetup = namedStep(job, "Install pinned Python");
    requireStepUses(
      violations,
      vulkanFile,
      job,
      "Install pinned Python",
      "actions/setup-python@v7.0.0",
    );
    add(
      violations,
      hasExactKeys(object(pythonSetup), ["name", "uses", "with", "env"])
        && hasExactKeys(object(pythonSetup?.with), ["python-version"])
        && object(pythonSetup?.with)["python-version"] === "3.13"
        && hasExactKeys(object(pythonSetup?.env), ["PSExecutionPolicyPreference"])
        && object(pythonSetup?.env).PSExecutionPolicyPreference === "Bypass",
      `${vulkanFile} must pin Python 3.13 with process-scoped script policy`,
    );
    add(
      violations,
      list(job.steps).at(1) === pythonSetup,
      `${vulkanFile} pinned Python must run immediately after checkout`,
    );
    const windowsPowerShellShell = `powershell -NoLogo -NoProfile -NonInteractive -ExecutionPolicy Bypass -Command ". '{0}'"`;
    for (const stepName of [
      "Capture host evidence",
      "Validate candidate-installed mode",
      "Capture source build tool evidence",
      "Install pinned Rust",
      "Build and package native CLI",
      "Authenticate calibration bundle producer",
      "Prove protected Windows Vulkan runtime",
      "Stage isolated candidate-managed Windows install",
      "Prove candidate-installed Windows Vulkan runtime",
    ]) {
      add(
        violations,
        namedStep(job, stepName)?.shell === windowsPowerShellShell,
        `${vulkanFile} ${stepName} must use built-in Windows PowerShell with process-scoped execution-policy bypass`,
      );
    }
    const validateCandidate = namedStep(job, "Validate candidate-installed mode");
    add(
      violations,
      validateCandidate?.if === "inputs.candidate_installed_proof",
      `${vulkanFile} candidate-installed validation must require explicit candidate mode`,
    );
    requireStepRun(violations, vulkanFile, job, "Validate candidate-installed mode", [
      'if ("${{ inputs.server_behavior_only }}" -ne "true")',
      "candidate_installed_proof requires server_behavior_only",
    ]);
    const sourceBuildTools = namedStep(job, "Capture source build tool evidence");
    add(
      violations,
      hasExactKeys(object(sourceBuildTools), ["name", "if", "shell", "run"])
        && sourceBuildTools?.if === "${{ !inputs.use_packaged_cli_artifact }}"
        && sourceBuildTools?.shell === windowsPowerShellShell,
      `${vulkanFile} source build tool evidence must remain source-only and fail closed`,
    );
    requireStepRun(violations, vulkanFile, job, "Capture source build tool evidence", [
      "CMAKE_GENERATOR=Ninja",
      "cmake --version",
      "ninja --version",
    ]);
    requireStepRun(violations, vulkanFile, job, "Prepare checksum-pinned embedded model", ["node scripts/prepare-embedded-model.mjs"]);
    const nativeBuild = namedStep(job, "Build and package native CLI");
    add(
      violations,
      hasExactKeys(object(nativeBuild?.env), ["VERSION", "CMAKE_GENERATOR"])
        && object(nativeBuild?.env).CMAKE_GENERATOR === windowsNativeGenerator,
      `${vulkanFile} source package build must use the Ninja native generator`,
    );
    requireStepRun(violations, vulkanFile, job, "Build and package native CLI", [
      "cargo build --release --locked -p codestory-cli",
      "package-codestory-release.py",
    ]);
    add(
      violations,
      namedStep(job, "Install pinned Rust")?.if
        === "${{ !inputs.use_packaged_cli_artifact || !inputs.server_behavior_only }}",
      `${vulkanFile} packaged server-behavior proof must skip unused Rust installation`,
    );
    add(
      violations,
      namedStep(job, "Build qualification driver")?.if
        === "${{ !inputs.server_behavior_only }}",
      `${vulkanFile} packaged server-behavior proof must skip the qualification driver`,
    );
    requireCalibrationProducerBoundary(
      violations,
      vulkanFile,
      job,
      "${{ !inputs.server_behavior_only }}",
    );
    const engine = namedStep(job, "Prove protected Windows Vulkan runtime");
    requireStepRun(violations, vulkanFile, job, "Prove protected Windows Vulkan runtime", [
      "--engine-policy accelerated",
      "--expected-backend Vulkan",
      "--offline",
      "--proof-tier protected_hardware",
      "--qualification-matrix-cell protected_windows_x64_vulkan",
      "--calibration-producer-run-id",
      "--calibration-producer-artifact",
      "--server-behavior-only",
      "Test-Path $qualityPath",
    ]);
    add(violations, object(engine?.env).CODESTORY_EMBED_ALLOW_CPU === "0", `${vulkanFile} engine proof must reject CPU fallback`);
    const engineRun = stepRun(job, "Prove protected Windows Vulkan runtime");
    add(
      violations,
      engineRun.includes("$calibrationArgs = @()")
        && engineRun.includes("@calibrationArgs")
        && engineRun.includes('$claimArgs = @("--server-behavior-only")')
        && occurrenceCount(engineRun, "--calibration-bundle") === 1,
      `${vulkanFile} server-behavior proof must omit calibration while qualification retains it`,
    );
    add(
      violations,
      engine?.if === "${{ !inputs.candidate_installed_proof }}",
      `${vulkanFile} protected Windows Vulkan proof must yield to the candidate-installed lane`,
    );
    const candidateStage = namedStep(job, "Stage isolated candidate-managed Windows install");
    add(
      violations,
      candidateStage?.if === "inputs.candidate_installed_proof",
      `${vulkanFile} candidate-managed staging must require explicit candidate mode`,
    );
    requireStepRun(violations, vulkanFile, job, "Stage isolated candidate-managed Windows install", [
      "--prepare-candidate-installed-proof",
      "--candidate-plugin-root-output",
      "--candidate-plugin-data-output",
      "--installed-plugin-attestation-output",
      "--candidate-producer-workflow-path",
      "gh api",
      "$run.head_repository.full_name",
      "$run.path",
      "$run.head_sha",
      "$run.run_attempt",
      "$env:CANDIDATE_PRODUCER_WORKFLOW_PATH",
      "[IO.Path]::GetPathRoot($env:GITHUB_WORKSPACE)",
      "cs-ci-",
      "Substring(0, 12)",
      "[IO.Path]::GetFullPath",
      "$env:GITHUB_WORKSPACE",
      "CODESTORY_CANDIDATE_WINDOWS_ROOT=",
    ]);
    const candidateProof = namedStep(job, "Prove candidate-installed Windows Vulkan runtime");
    add(
      violations,
      candidateProof?.if === "inputs.candidate_installed_proof",
      `${vulkanFile} candidate-installed Vulkan proof must require explicit candidate mode`,
    );
    requireStepRun(violations, vulkanFile, job, "Prove candidate-installed Windows Vulkan runtime", [
      "--proof-tier installed_runtime",
      "--engine-policy accelerated",
      "--expected-backend Vulkan",
      "--installed-plugin-attestation",
      "--installed-plugin-data",
      "--candidate-producer-workflow-path",
      "$env:CANDIDATE_PRODUCER_WORKFLOW_PATH",
      "--server-behavior-only",
      "--expected-source-sha",
      "--expected-source-tree",
      "$env:CODESTORY_CANDIDATE_WINDOWS_ROOT",
      "$env:TEMP = $proofTemp",
      "$env:TMP = $proofTemp",
      "$env:TMPDIR = $proofTemp",
    ]);
    const candidateProofRun = stepRun(
      job,
      "Prove candidate-installed Windows Vulkan runtime",
    );
    add(
      violations,
      !candidateProofRun.includes("calibration")
        && !candidateProofRun.includes("--ground-only")
        && !candidateProofRun.includes("--engine-policy cpu_explicit"),
      `${vulkanFile} candidate-installed Vulkan proof must be bounded accelerated runtime proof`,
    );
    add(
      violations,
      object(candidateProof?.env).CODESTORY_EMBED_ALLOW_CPU === "0",
      `${vulkanFile} candidate-installed proof must reject CPU fallback`,
    );
    const candidateUpload = namedStep(job, "Upload candidate-installed Windows proof");
    add(
      violations,
      candidateUpload?.uses === "actions/upload-artifact@v7.0.1"
        && String(candidateUpload?.if ?? "").includes("always()")
        && String(candidateUpload?.if ?? "").includes("inputs.candidate_installed_proof")
        && object(candidateUpload?.with).name
          === "candidate-installed-windows-${{ inputs.version }}-attempt-${{ github.run_attempt }}"
        && object(candidateUpload?.with).path === "target/candidate-installed-windows"
        && object(candidateUpload?.with)["if-no-files-found"] === "error",
      `${vulkanFile} candidate-installed proof must retain one attempt-scoped artifact`,
    );
    requireStepRun(violations, vulkanFile, job, "Emit authenticated Vulkan release cell", [
      "codestory-release-cell-manifest.mjs produce",
      "accelerator_execution:windows-x64-vulkan",
      "--producer-job packaged-vulkan",
    ]);
    const releaseCell = namedStep(job, "Emit authenticated Vulkan release cell");
    add(
      violations,
      releaseCell?.if === "inputs.emit_release_cells",
      `${vulkanFile} accelerated proof must retain the authenticated Vulkan release cell`,
    );
    const vulkanCellUpload = namedStep(job, "Upload authenticated Vulkan release cell");
    add(
      violations,
      vulkanCellUpload?.uses === "actions/upload-artifact@v7.0.1"
        && String(vulkanCellUpload?.if ?? "").includes("success()")
        && String(vulkanCellUpload?.if ?? "").includes("inputs.emit_release_cells")
        && !String(vulkanCellUpload?.if ?? "").includes("!inputs.server_behavior_only"),
      `${vulkanFile} Vulkan release cell must be a success-only retained artifact`,
    );
    requireStepRun(violations, vulkanFile, job, "Emit authenticated candidate-installed Windows release cell", [
      "candidate_installed_behavior:windows-x64",
      "--producer-job packaged-vulkan",
      "candidate_managed_plugin",
    ]);
    forbidStepRun(
      violations,
      vulkanFile,
      job,
      "Emit authenticated Vulkan release cell",
      ["calibration"],
    );
    forbidStepRun(
      violations,
      vulkanFile,
      job,
      "Emit authenticated candidate-installed Windows release cell",
      ["calibration"],
    );
    requireStepUses(
      violations,
      vulkanFile,
      job,
      "Upload authenticated candidate-installed Windows release cell",
      "actions/upload-artifact@v7.0.1",
    );
  }

  const linuxVulkanFile = "linux-vulkan-proof.yml";
  const linuxVulkan = workflows.get(linuxVulkanFile);
  if (!linuxVulkan) {
    violations.push(`${linuxVulkanFile} must exist`);
  } else {
    add(
      violations,
      trigger(linuxVulkan, "workflow_call") !== undefined
        && trigger(linuxVulkan, "workflow_dispatch") !== undefined,
      `${linuxVulkanFile} must support reusable and manual proof`,
    );
    for (const event of ["workflow_call", "workflow_dispatch"]) {
      for (const key of ["calibration_bundle_artifact", "calibration_bundle_run_id"]) {
        requireOptionalStringInput(violations, linuxVulkanFile, linuxVulkan, event, key);
      }
      add(
        violations,
        at(linuxVulkan, "on", event, "inputs", "candidate_installed_only") === undefined,
        `${linuxVulkanFile} ${event} must not define candidate_installed_only`,
      );
    }
    const candidateInput = object(at(
      linuxVulkan,
      "on",
      "workflow_call",
      "inputs",
      "candidate_installed_proof",
    ));
    add(
      violations,
      candidateInput.required === false
        && candidateInput.type === "boolean"
        && candidateInput.default === false,
      `${linuxVulkanFile} reusable candidate-installed proof must be an explicit opt-in`,
    );
    add(
      violations,
      at(
        linuxVulkan,
        "on",
        "workflow_dispatch",
        "inputs",
        "candidate_producer_workflow_path",
        "default",
      ) === ".github/workflows/packaged-platform-pr.yml",
      `${linuxVulkanFile} manual candidate proof must trust the package-producing workflow`,
    );
    const job = requireJob(violations, linuxVulkanFile, linuxVulkan, "packaged-vulkan");
    requireStepUses(
      violations,
      linuxVulkanFile,
      job,
      "Install pinned Python",
      "actions/setup-python@v7.0.0",
    );
    requireStepRun(violations, linuxVulkanFile, job, "Capture Linux Vulkan host evidence", [
      "uname -m",
      "vulkaninfo --summary",
      "test \"$(uname -m)\" = x86_64",
    ]);
    const validateCandidate = namedStep(job, "Validate candidate-installed mode");
    add(
      violations,
      validateCandidate?.if === "inputs.candidate_installed_proof"
        && validateCandidate?.shell === "bash",
      `${linuxVulkanFile} candidate-installed validation must require explicit candidate mode`,
    );
    requireStepRun(violations, linuxVulkanFile, job, "Validate candidate-installed mode", [
      'test "${{ inputs.server_behavior_only }}" = true',
    ]);
    const packageDownload = namedStep(job, "Download exact Linux package");
    add(
      violations,
      packageDownload?.uses === "actions/download-artifact@v8.0.1"
        && object(packageDownload.with).name === "codestory-cli-linux-x64",
      `${linuxVulkanFile} must consume the graph-declared Linux x64 package`,
    );
    requireCalibrationProducerBoundary(
      violations,
      linuxVulkanFile,
      job,
      "${{ !inputs.server_behavior_only }}",
    );
    const engine = namedStep(job, "Prove offline Linux Vulkan retrieval");
    requireStepRun(violations, linuxVulkanFile, job, "Prove offline Linux Vulkan retrieval", [
      "--engine-policy accelerated",
      "--expected-backend Vulkan",
      "--offline",
      "--proof-tier protected_hardware",
      "--qualification-matrix-cell protected_linux_x64_vulkan",
      "--calibration-producer-run-id",
      "--calibration-producer-artifact",
      "--server-behavior-only",
    ]);
    add(
      violations,
      object(engine?.env).CODESTORY_EMBED_ALLOW_CPU === "0",
      `${linuxVulkanFile} protected proof must reject CPU fallback`,
    );
    const engineRun = stepRun(job, "Prove offline Linux Vulkan retrieval");
    add(
      violations,
      engine?.if === "${{ !inputs.candidate_installed_proof }}",
      `${linuxVulkanFile} protected Linux Vulkan proof must yield to the candidate-installed lane`,
    );
    add(
      violations,
      engineRun.includes("calibration_args=()")
        && engineRun.includes('"${calibration_args[@]}"')
        && engineRun.includes('claim_args=(--server-behavior-only)')
        && occurrenceCount(engineRun, "--calibration-bundle") === 1,
      `${linuxVulkanFile} server-behavior proof must omit calibration while qualification retains it`,
    );
    requireStepRun(violations, linuxVulkanFile, job, "Stage isolated candidate-managed Linux install", [
      "--prepare-candidate-installed-proof",
      "--candidate-plugin-root-output",
      "--candidate-plugin-data-output",
      "--installed-plugin-attestation-output",
      "--candidate-producer-workflow-path",
      "gh api",
      "$RUNNER_TEMP/codestory-candidate-installed-linux.",
      "CODESTORY_CANDIDATE_LINUX_ROOT=",
    ]);
    add(
      violations,
      namedStep(job, "Stage isolated candidate-managed Linux install")?.if
        === "inputs.candidate_installed_proof",
      `${linuxVulkanFile} candidate-managed staging must require explicit candidate mode`,
    );
    const candidate = namedStep(job, "Prove candidate-installed Linux Vulkan runtime");
    requireStepRun(violations, linuxVulkanFile, job, "Prove candidate-installed Linux Vulkan runtime", [
      "--engine-policy accelerated",
      "--expected-backend Vulkan",
      "--proof-tier installed_runtime",
      "--installed-plugin-attestation",
      "--installed-plugin-data",
      "--server-behavior-only",
    ]);
    add(
      violations,
      candidate?.if === "inputs.candidate_installed_proof",
      `${linuxVulkanFile} candidate-installed Vulkan proof must require explicit candidate mode`,
    );
    add(
      violations,
      object(candidate?.env).CODESTORY_EMBED_ALLOW_CPU === "0",
      `${linuxVulkanFile} candidate-installed proof must reject CPU fallback`,
    );
    forbidStepRun(
      violations,
      linuxVulkanFile,
      job,
      "Prove candidate-installed Linux Vulkan runtime",
      ["calibration", "--ground-only", "--engine-policy cpu_explicit"],
    );
    requireStepRun(violations, linuxVulkanFile, job, "Emit authenticated Linux Vulkan release cells", [
      "accelerator_execution:linux-x64-vulkan",
      "retrieval_readiness:linux-x64",
      "candidate_installed_behavior:linux-x64",
      "--producer-job packaged-vulkan",
    ]);
    forbidStepRun(
      violations,
      linuxVulkanFile,
      job,
      "Emit authenticated Linux Vulkan release cells",
      ["calibration"],
    );
    for (const name of [
      "Upload authenticated Linux accelerator release cell",
      "Upload authenticated Linux retrieval release cell",
      "Upload authenticated candidate-installed Linux release cell",
    ]) {
      requireStepUses(
        violations,
        linuxVulkanFile,
        job,
        name,
        "actions/upload-artifact@v7.0.1",
      );
    }
  }

  const statsFile = "repo-scale-stats.yml";
  const stats = workflows.get(statsFile);
  if (!stats) {
    violations.push(`${statsFile} must exist`);
  } else {
    const job = requireJob(violations, statsFile, stats, "stats");
    requireStepRun(violations, statsFile, job, "Prepare checksum-pinned embedded model", ["node scripts/prepare-embedded-model.mjs"]);
    requireStepRun(violations, statsFile, job, "Build the release CLI", ["cargo build --release --locked -p codestory-cli"]);
    requireStepRun(violations, statsFile, job, "Run mandatory repo-scale stats once", ["cargo test --locked -p codestory-cli --test codestory_repo_e2e_stats -- --ignored --nocapture"]);
    requireStepUses(violations, statsFile, job, "Upload repo-scale stats output", "actions/upload-artifact@v7.0.1");
  }

  const retrieval = workflows.get(retrievalFile);
  if (!retrieval) {
    violations.push(`${retrievalFile} must exist`);
  } else {
    for (const violation of windowsManifestProofPolicyViolations(retrieval)) {
      violations.push(`${retrievalFile} ${violation}`);
    }
  }

  const guardFile = "main-branch-source-guard.yml";
  const guard = workflows.get(guardFile);
  if (!guard) {
    violations.push(`${guardFile} must exist`);
  } else {
    add(violations, includesAll(at(guard, "on", "pull_request", "branches"), ["main"]), `${guardFile} must guard main`);
    const job = requireJob(violations, guardFile, guard, "enforce-source-branch");
    const step = namedStep(job, "Require dev/codestory-next source branch");
    add(violations, object(step?.env).HEAD_REPO !== undefined && object(step?.env).BASE_REPO !== undefined, `${guardFile} must compare source and base repository identity`);
    add(violations, String(step?.run ?? "").includes("dev/codestory-next"), `${guardFile} must require the dev source branch`);
  }
}

function permissionMapMatches(actualValue, expectedValue) {
  const actual = object(actualValue);
  const expected = object(expectedValue);
  return sameMembers(Object.keys(actual), Object.keys(expected))
    && Object.entries(expected).every(([key, value]) => actual[key] === value);
}

function findNamedStep(workflow, name) {
  for (const job of Object.values(object(workflow.jobs))) {
    const found = namedStep(job, name);
    if (found) return found;
  }
  return undefined;
}

export function releaseWorkflowContractViolations(
  workflows,
  graph = loadReleaseClaimGraph(repositoryRoot),
) {
  const violations = [];
  const policy = graph.workflow_policy;
  for (const contract of policy.protected_jobs) {
    const workflow = workflows.get(contract.workflow);
    const job = object(at(workflow, "jobs", contract.job));
    const effectivePermissions = job.permissions === undefined ? workflow?.permissions : job.permissions;
    add(
      violations,
      JSON.stringify(job["runs-on"]) === JSON.stringify(contract.runner),
      `[runner_labels] ${contract.workflow} job ${contract.job} must use ${JSON.stringify(contract.runner)}`,
    );
    add(
      violations,
      job.environment === contract.environment,
      `[protected_environment] ${contract.workflow} job ${contract.job} must use ${contract.environment}`,
    );
    add(
      violations,
      permissionMapMatches(effectivePermissions, contract.permissions),
      `[permissions_secrets] ${contract.workflow} job ${contract.job} effective permissions must exactly match the release claim graph`,
    );
    add(
      violations,
      sameMembers(Object.keys(object(at(workflow, "on", "workflow_call", "secrets"))), contract.secrets),
      `[permissions_secrets] ${contract.workflow} callable secrets must exactly match the release claim graph`,
    );
    const reusableRef = `./.github/workflows/${contract.workflow}`;
    for (const [callerFile, callerWorkflow] of workflows) {
      for (const [callerJobName, callerJobValue] of Object.entries(object(callerWorkflow.jobs))) {
        const callerJob = object(callerJobValue);
        if (callerJob.uses !== reusableRef) continue;
        add(
          violations,
          callerJob.secrets !== "inherit",
          `[permissions_secrets] ${callerFile} job ${callerJobName} must not use secrets: inherit for ${contract.workflow}`,
        );
        if (callerJob.secrets !== undefined && callerJob.secrets !== "inherit") {
          const forwarded = Object.keys(object(callerJob.secrets));
          const undeclared = forwarded.filter((secret) => !contract.secrets.includes(secret));
          add(
            violations,
            undeclared.length === 0,
            `[permissions_secrets] ${callerFile} job ${callerJobName} forwards undeclared secrets to ${contract.workflow}: ${undeclared.join(", ")}`,
          );
        }
      }
    }
  }

  for (const file of policy.artifact_workflows) {
    const workflow = workflows.get(file);
    let uploadCount = 0;
    for (const [jobName, job] of Object.entries(object(workflow?.jobs))) {
      for (const [index, step] of list(job?.steps).entries()) {
        if (step?.uses !== "actions/upload-artifact@v7.0.1") continue;
        uploadCount += 1;
        add(
          violations,
          object(step.with)["retention-days"] === policy.artifact_retention_days,
          `[artifact_retention] ${file} jobs.${jobName}.steps.${index} must retain release evidence for ${policy.artifact_retention_days} days`,
        );
      }
    }
    add(violations, uploadCount > 0, `[artifact_retention] ${file} must upload its release-significant evidence`);
  }

  const release = workflows.get("release.yml");
  for (const jobName of policy.release_chain.exact_sha_jobs) {
    add(
      violations,
      object(at(release, "jobs", jobName, "with")).ref === policy.promotion.exact_sha_expression,
      `[exact_sha] release.yml job ${jobName} must receive ${policy.promotion.exact_sha_expression}`,
    );
  }
  for (const [jobName, expectedNeeds] of Object.entries(policy.release_chain.dependencies)) {
    add(
      violations,
      sameMembers(needs(at(release, "jobs", jobName)), expectedNeeds),
      `[promotion_boundary] release.yml job ${jobName} dependencies must match the release claim graph`,
    );
  }
  const sourceGuard = workflows.get("main-branch-source-guard.yml");
  const sourceGuardRun = executableRunText(String(findNamedStep(sourceGuard, "Require dev/codestory-next source branch")?.run ?? ""));
  add(
    violations,
    includesAll(at(sourceGuard, "on", "pull_request", "branches"), [policy.promotion.release_branch])
      && sourceGuardRun.includes(policy.promotion.source_branch),
    `[promotion_boundary] main-branch source guard must require ${policy.promotion.source_branch} into ${policy.promotion.release_branch}`,
  );
  add(
    violations,
    includesAll(at(workflows.get("auto-release.yml"), "on", "push", "branches"), [policy.promotion.release_branch]),
    `[promotion_boundary] auto-release.yml must run from ${policy.promotion.release_branch}`,
  );
  const matrixViolations = [];
  validatePackageMatrixExpression(
    matrixViolations,
    at(workflows.get("packaged-platform-proof.yml"), "jobs", "build", "strategy", "matrix"),
    graph,
  );
  violations.push(...matrixViolations.map((message) => `[target_matrix] ${message}`));

  for (const file of policy.promotion.label_routed_workflows) {
    const workflow = workflows.get(file);
    add(
      violations,
      sameMembers(at(workflow, "on", "pull_request", "types"), policy.promotion.required_events),
      `[proof_identity] ${file} must use only ${policy.promotion.required_events.join(" and ")} pull-request events`,
    );
    add(
      violations,
      String(at(workflow, "concurrency", "group") ?? "").includes(policy.promotion.proof_run_sha_expression),
      `[proof_identity] ${file} concurrency must bind ${policy.promotion.proof_run_sha_expression}`,
    );
    const resolver = findNamedStep(workflow, "Resolve trusted exact head");
    const run = executableRunText(String(resolver?.run ?? ""));
    add(
      violations,
      run.includes("current_head") && run.includes("EVENT_HEAD_SHA"),
      `[proof_identity] ${file} must resolve the current head and compare its exact SHA before executing labeled work`,
    );
  }
  return violations;
}

function validateReleaseCellUploadOwnership(workflows, violations) {
  const evidenceFile = releaseEvidenceWorkflowRef.slice(
    releaseEvidenceWorkflowRef.lastIndexOf("/") + 1,
  );
  const actual = [];
  for (const [file, workflow] of workflows) {
    for (const [jobId, jobValue] of Object.entries(object(workflow.jobs))) {
      for (const step of Array.isArray(jobValue.steps) ? jobValue.steps : []) {
        if (step?.uses !== "actions/upload-artifact@v7.0.1") continue;
        const artifactName = String(object(step.with).name ?? "");
        if (artifactName.startsWith("release-cell-")) {
          actual.push(`${file}/${jobId}/${artifactName}`);
        }
      }
    }
  }
  const expected = [
    "source-proof.yml/full-source-gate/release-cell-prepublish-source-attempt-${{ github.run_attempt }}",
    "packaged-platform-proof.yml/build/release-cell-prepublish-package-${{ matrix.asset_target }}-attempt-${{ github.run_attempt }}",
    "macos-metal-proof.yml/packaged-metal/release-cell-prepublish-macos-arm64-metal-attempt-${{ github.run_attempt }}",
    "macos-metal-proof.yml/packaged-metal/release-cell-postpublish-retrieval-macos-arm64-attempt-${{ github.run_attempt }}",
    "macos-metal-proof.yml/packaged-metal/release-cell-prepublish-candidate-installed-macos-arm64-attempt-${{ github.run_attempt }}",
    "windows-vulkan-proof.yml/packaged-vulkan/release-cell-prepublish-windows-x64-vulkan-attempt-${{ github.run_attempt }}",
    "windows-vulkan-proof.yml/packaged-vulkan/release-cell-postpublish-retrieval-windows-x64-attempt-${{ github.run_attempt }}",
    "windows-vulkan-proof.yml/packaged-vulkan/release-cell-prepublish-candidate-installed-windows-x64-attempt-${{ github.run_attempt }}",
    "linux-vulkan-proof.yml/packaged-vulkan/release-cell-prepublish-linux-x64-vulkan-attempt-${{ github.run_attempt }}",
    "linux-vulkan-proof.yml/packaged-vulkan/release-cell-postpublish-retrieval-linux-x64-attempt-${{ github.run_attempt }}",
    "linux-vulkan-proof.yml/packaged-vulkan/release-cell-prepublish-candidate-installed-linux-x64-attempt-${{ github.run_attempt }}",
    "post-publish-release-smoke.yml/smoke/release-cell-postpublish-${{ matrix.asset_target }}-attempt-${{ github.run_attempt }}",
  ];
  add(
    violations,
    JSON.stringify(actual.sort()) === JSON.stringify(expected.sort()),
    "release-cell Actions artifact names must have one graph-owned producer job and attempt suffix",
  );
}

function validateReleaseArtifactRerunSafety(workflows, violations) {
  const evidenceFile = releaseEvidenceWorkflowRef.slice(
    releaseEvidenceWorkflowRef.lastIndexOf("/") + 1,
  );
  const releaseChainWorkflows = new Set([
    "source-proof.yml",
    evidenceFile,
    "packaged-platform-proof.yml",
    "macos-metal-proof.yml",
    "windows-vulkan-proof.yml",
    "linux-vulkan-proof.yml",
    "post-publish-release-smoke.yml",
    "release.yml",
  ]);
  const replaceableStableIntermediates = new Map([
    [`${evidenceFile}/measure/Upload release evidence`, {
      name: "release-evidence-${{ inputs.ref }}",
      path: "target/release-evidence",
    }],
    ["packaged-platform-proof.yml/build/Upload hosted Linux calibration runs", {
      name: "embedding-calibration-linux-${{ inputs.version }}",
      path: "target/calibration-runs/linux",
    }],
    ["packaged-platform-proof.yml/build/Upload release asset", {
      name: "codestory-cli-${{ matrix.asset_target }}",
      path: "target/release-dist/*.tar.gz\ntarget/release-dist/*.zip\ntarget/release-dist/*.sha256\ntarget/release-dist/SHA256SUMS.txt\n",
    }],
    ["macos-metal-proof.yml/packaged-metal/Upload Metal calibration runs", {
      name: "embedding-calibration-macos-${{ inputs.version }}",
      path: "target/calibration-runs/macos",
    }],
    ["release.yml/pre-publish-closeout/Upload accepted pre-publish closeout", {
      name: "release-closeout-pre-publish-${{ needs.preflight.outputs.version }}-${{ github.sha }}",
      path: "target/release-closeout/trusted-pre-publish-producers.json\ntarget/release-closeout/pre_publish\n",
    }],
  ]);
  const observedStableIntermediates = new Set();
  for (const [file, workflow] of workflows) {
    if (!releaseChainWorkflows.has(file)) continue;
    for (const [jobId, jobValue] of Object.entries(object(workflow.jobs))) {
      for (const step of list(jobValue?.steps)) {
        if (step?.uses !== "actions/upload-artifact@v7.0.1") continue;
        const upload = object(step.with);
        const artifactName = String(upload.name ?? "");
        const uploadKey = `${file}/${jobId}/${step.name ?? ""}`;
        const attemptQualified = artifactName.includes("${{ github.run_attempt }}");
        const expectedStable = replaceableStableIntermediates.get(uploadKey);
        const stableIntermediateMatches = expectedStable !== undefined
          && !observedStableIntermediates.has(uploadKey)
          && artifactName === expectedStable.name
          && String(upload.path ?? "") === expectedStable.path
          && upload.overwrite === true;
        if (expectedStable !== undefined) {
          observedStableIntermediates.add(uploadKey);
        }
        add(
          violations,
          expectedStable !== undefined
            ? stableIntermediateMatches
            : attemptQualified && upload.overwrite !== true,
          `${file} job ${jobId} upload ${step.name ?? artifactName} must be immutable and attempt-qualified unless it is an explicitly allowlisted stable intermediate`,
        );
      }
    }
  }
  for (const uploadKey of replaceableStableIntermediates.keys()) {
    add(
      violations,
      observedStableIntermediates.has(uploadKey),
      `${uploadKey} stable intermediate upload must exist exactly once with its policy-owned artifact name and path`,
    );
  }
}

// Cargo test-name filters are substring matches: a filter that names nothing selects zero tests and
// still exits 0, so a renamed test turns its proof lane green without running anything. `--exact`
// does not help — libtest also exits 0 when an exact filter matches nothing. These names are
// therefore checked statically against the crate sources they claim to select.
const cargoValueOptions = new Set([
  "-p",
  "--package",
  "--test",
  "--bench",
  "--example",
  "--bin",
  "--features",
  "--target",
  "--target-dir",
  "--manifest-path",
  "--profile",
  "--jobs",
  "-j",
]);

const expressionPlaceholder = "__CODESTORY_GITHUB_EXPRESSION__";
const harnessValueOptions = new Set(["--color", "--format", "--skip", "--test-threads"]);

function cargoTestFilterNames(line) {
  // GitHub expressions expand at run time; treat each as one opaque token so an option that takes a
  // value consumes it instead of leaving `matrix.foo` behind as a bare positional.
  const tokens = line
    .replace(/\$\{\{.*?\}\}/gu, expressionPlaceholder)
    .trim()
    .split(/\s+/u)
    .filter(token => token !== "\\");
  const start = tokens.findIndex(token => token === "test");
  if (start < 0) return { package: null, filters: [], exact: false };
  const separator = tokens.indexOf("--", start + 1);
  const cargoTokens = tokens.slice(start + 1, separator < 0 ? undefined : separator);
  const harnessTokens = separator < 0 ? [] : tokens.slice(separator + 1);

  let packageName = null;
  const filters = [];
  for (let index = 0; index < cargoTokens.length; index += 1) {
    const token = cargoTokens[index];
    if (cargoValueOptions.has(token)) {
      if (token === "-p" || token === "--package") packageName = cargoTokens[index + 1] ?? null;
      index += 1;
      continue;
    }
    if (token.startsWith("-")) {
      const [name, value] = token.split("=");
      if ((name === "-p" || name === "--package") && value) packageName = value;
      continue;
    }
    if (token.includes(expressionPlaceholder)) continue;
    // Cargo accepts at most one positional TESTNAME filter.
    filters.push(token);
    break;
  }
  for (let index = 0; index < harnessTokens.length; index += 1) {
    const token = harnessTokens[index];
    if (harnessValueOptions.has(token)) {
      index += 1;
      continue;
    }
    if (token.startsWith("-") || token.includes(expressionPlaceholder)) continue;
    filters.push(token);
  }
  return { package: packageName, filters, exact: harnessTokens.includes("--exact") };
}

function crateDirectories() {
  const crateRoot = path.join(repositoryRoot, "crates");
  const directories = new Map();
  if (!fs.existsSync(crateRoot)) return directories;
  for (const entry of fs.readdirSync(crateRoot, { withFileTypes: true })) {
    if (!entry.isDirectory()) continue;
    const manifest = path.join(crateRoot, entry.name, "Cargo.toml");
    if (!fs.existsSync(manifest)) continue;
    const name = /^\s*name\s*=\s*"([^"]+)"/mu.exec(fs.readFileSync(manifest, "utf8"))?.[1];
    if (name) directories.set(name, path.join(crateRoot, entry.name));
  }
  return directories;
}

function rustSourceIdentifiers(directory) {
  const identifiers = new Set();
  const stack = [directory];
  while (stack.length > 0) {
    const current = stack.pop();
    let entries;
    try {
      entries = fs.readdirSync(current, { withFileTypes: true });
    } catch {
      continue;
    }
    for (const entry of entries) {
      const entryPath = path.join(current, entry.name);
      if (entry.isDirectory()) {
        if (entry.name !== "target") stack.push(entryPath);
        continue;
      }
      if (!entry.name.endsWith(".rs")) continue;
      const source = fs.readFileSync(entryPath, "utf8");
      for (const match of source.matchAll(/\b(?:fn|mod)\s+([A-Za-z_][A-Za-z0-9_]*)/gu)) {
        identifiers.add(match[1]);
      }
    }
  }
  return identifiers;
}

export function validateCargoTestFilters(
  workflows,
  violations,
  directories = crateDirectories(),
  readIdentifiers = rustSourceIdentifiers,
) {
  const identifierCache = new Map();
  const identifiersFor = packageName => {
    if (!identifierCache.has(packageName)) {
      const directory = directories.get(packageName);
      identifierCache.set(packageName, directory ? readIdentifiers(directory) : null);
    }
    return identifierCache.get(packageName);
  };

  for (const [file, workflow] of workflows) {
    for (const [jobName, rawJob] of Object.entries(object(workflow.jobs))) {
      for (const [stepIndex, step] of list(object(rawJob).steps).entries()) {
        if (typeof step?.run !== "string") continue;
        for (const { line, number } of executableCargoLines(step.run)) {
          if (!/^\s*(?:[A-Z_][A-Z0-9_]*=\S+\s+)*(?:sudo\s+)?cargo\s+test\b/u.test(line)) continue;
          const { package: packageName, filters, exact } = cargoTestFilterNames(line);
          if (filters.length === 0) continue;
          // A workspace-wide run resolves names across every crate, which this guard does not model.
          if (!packageName) continue;
          const identifiers = identifiersFor(packageName);
          const location = `${file} jobs.${jobName}.steps.${stepIndex}.run:${number}`;
          if (!identifiers) {
            violations.push(`${location} names unknown package ${packageName}`);
            continue;
          }
          for (const filter of filters) {
            for (const segment of filter.split("::").filter(Boolean)) {
              // Without `--exact` cargo matches substrings, so mirror that rather than demanding a
              // whole identifier: `publication_transitions_...` legitimately selects both the
              // `full_` and `incremental_` variants.
              const resolved = exact
                ? identifiers.has(segment)
                : [...identifiers].some(identifier => identifier.includes(segment));
              add(
                violations,
                resolved,
                `${location} cargo test filter "${filter}" selects no test: ${segment} matches no fn or mod in ${packageName}`,
              );
            }
          }
        }
      }
    }
  }
}

export function validatePluginRelease(workflows, violations) {
  const file = "plugin-release.yml";
  const workflow = workflows.get(file);
  if (!workflow) {
    violations.push(`${file} must exist`);
    return;
  }
  const scalars = scalarStrings(workflow);
  add(violations, hasExactKeys(object(workflow.on), ["workflow_call"]), `${file} must be callable only`);
  add(
    violations,
    !JSON.stringify(workflow).includes("secrets"),
    `${file} must not receive or forward secrets: nothing is built or signed on the plugin lane`,
  );
  walk(workflow, (key, value) => {
    if (/^APPLE_/u.test(key) || (typeof value === "string" && /\bAPPLE_[A-Z0-9_]+\b/u.test(value))) {
      violations.push(`${file} must never reference Apple signing material`);
    }
  });
  const jobs = object(workflow.jobs);
  add(
    violations,
    hasExactKeys(jobs, ["workflow-policy", "preflight", "plugin-proof", "publish", "post-publish-smoke"]),
    `${file} must keep its exact five-job plugin lane`,
  );
  for (const [name, job] of Object.entries(jobs)) {
    const permissions = object(job).permissions;
    add(
      violations,
      name === "publish" ? object(permissions).contents === "write" : permissions === undefined,
      `${file} only the publish job may hold write permission`,
    );
  }
  const preflight = object(jobs.preflight);
  requireStepRun(violations, file, preflight, "Validate release authority", [
    "auto-release.yml@refs/heads/main",
    "repos/$GITHUB_REPOSITORY/git/ref/heads/main",
  ]);
  requireStepRun(violations, file, preflight, "Validate plugin-lane version synchronization", [
    "check-codestory-release.py",
    "--lane plugin",
  ]);
  requireStepRun(violations, file, preflight, "Bind the pin to the published CLI release", [
    "cli-version.json",
    "SHA256SUMS.txt",
  ]);
  requireStepRun(violations, file, preflight, "Refuse a changed tool surface", [
    "generated-mcp-catalog.json",
  ]);
  requireStepRun(violations, file, object(jobs["plugin-proof"]), "Provision the pinned CLI end to end", [
    "scripts/prove-plugin-pinned-provision.mjs",
  ]);
  requireStepRun(violations, file, object(jobs.publish), "Re-verify main before tagging", [
    "repos/$GITHUB_REPOSITORY/git/ref/heads/main",
  ]);
  add(
    violations,
    sameStrings(nonCommentLines(object(jobs.publish).needs === undefined ? "" : ""), []) ||
      JSON.stringify(object(jobs.publish).needs) === JSON.stringify(["preflight", "plugin-proof"]),
    `${file} publish must wait on preflight and plugin proof`,
  );
  add(
    violations,
    !scalars.some((value) => /cargo\s+(?:build|test)/u.test(value)),
    `${file} must not build native code`,
  );

  const auto = workflows.get("auto-release.yml");
  const pluginCaller = object(at(auto, "jobs", "plugin-release"));
  add(
    violations,
    pluginCaller.uses === "./.github/workflows/plugin-release.yml"
      && String(pluginCaller.if ?? "").includes("release_lane == 'plugin'")
      && pluginCaller.secrets === undefined,
    "auto-release.yml must route the plugin lane without forwarding secrets",
  );
  add(
    violations,
    String(object(at(auto, "jobs", "release")).if ?? "").includes("release_lane == 'native'"),
    "auto-release.yml native release must be gated on the native lane",
  );
}

export function validateWorkflows(workflows, graph = loadReleaseClaimGraph(repositoryRoot)) {
  const violations = [];
  for (const [file, workflow] of workflows) {
    violations.push(...basicWorkflowViolations(file, workflow));
  }
  validateCargoTestFilters(workflows, violations);
  validatePluginRelease(workflows, violations);
  validateLockedSetupSurfaces(violations);
  validateIssueWorkflows(workflows, violations);
  validatePluginAndDraftWorkflows(workflows, violations, graph);
  validateReleaseCoordinator(workflows, violations, graph);
  validatePackagedProof(workflows, violations, graph);
  validatePostPublish(workflows, violations);
  validatePackagedCoordinator(workflows, violations, graph);
  validateRemainingWorkflows(workflows, violations);
  validateReleaseCellUploadOwnership(workflows, violations);
  validateReleaseArtifactRerunSafety(workflows, violations);
  violations.push(...releaseWorkflowContractViolations(workflows, graph));
  return violations;
}

function main() {
  let workflows;
  try {
    workflows = loadWorkflows();
  } catch (error) {
    console.error(`Workflow YAML parse failed: ${error.message}`);
    process.exit(1);
  }
  const violations = validateWorkflows(workflows);
  if (violations.length > 0) {
    console.error(violations.join("\n"));
    process.exit(1);
  }
  console.log("Workflow policy passed: parsed workflow structure satisfies repository contracts.");
}

if (process.argv[1] && fileURLToPath(import.meta.url) === path.resolve(process.argv[1])) {
  main();
}
