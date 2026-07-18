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

function requireResolverSemantics(violations, file, job, exactLines) {
  const run = executableRunText(stepRun(job, "Resolve trusted exact head"));
  const lines = run.split(/\r?\n/u).map(line => line.trim()).filter(Boolean);
  add(
    violations,
    !lines.some(line => /^true\s*\|\|/u.test(line) || /^if\s+false(?:\s|;|&&)/u.test(line)),
    `${file} resolver must not short-circuit trusted branch or SHA checks`,
  );
  for (const expected of exactLines) {
    add(
      violations,
      lines.includes(expected),
      `${file} resolver must execute exact line ${expected}`,
    );
  }
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
const draftProofCommands = [
  "cargo test --locked -p codestory-llama-sys --test native_staging",
  "cargo test --locked -p codestory-llama-sys --test model_staging",
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
const cacheSaveKey = "${{ steps.cargo-cache-restore.outputs.cache-primary-key }}";
const draftWorkflowPaths = [
  "Cargo.lock",
  "Cargo.toml",
  "crates/**",
  ".github/scripts/check-runtime-config-boundary.mjs",
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
  { name: "Capture Rust cache identity", keys: ["name", "id", "shell", "run"] },
  {
    name: "Restore Cargo inputs and output",
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
];
const draftRunCommands = new Map([
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

export function managedPluginViolations(job, archiveFragment) {
  const violations = [];
  const strategy = object(job.strategy);
  const matrix = strategy.matrix;
  const step = namedStep(job, "Prove managed plugin handoff");
  add(violations, strategy["fail-fast"] === false, "managed plugin matrix must set fail-fast to false");
  add(violations, job.if === undefined, "managed plugin job must not be conditional");
  add(violations, job["continue-on-error"] === undefined, "managed plugin job must not continue on error");
  add(
    violations,
    typeof matrix === "string" || object(matrix).exclude === undefined,
    "managed plugin matrix must not exclude cells",
  );
  add(violations, step !== undefined, "managed plugin proof step is missing");
  add(violations, step?.if === undefined, "managed plugin proof step must not be conditional");
  add(
    violations,
    step?.["continue-on-error"] === undefined,
    "managed plugin proof step must not continue on error",
  );
  add(
    violations,
    object(step?.env).CODESTORY_EMBED_ALLOW_CPU === "1",
    "managed plugin proof step must explicitly allow CPU evidence",
  );
  const run = executableRunText(typeof step?.run === "string" ? step.run : "");
  for (const fragment of [
    "python .github/scripts/check-packaged-agent-proof.py",
    archiveFragment,
    "--plugin-handoff",
    "--engine-policy cpu_explicit",
    "--expected-backend CPU",
    "--offline",
  ]) {
    add(violations, run.includes(fragment), `managed plugin proof step must run ${fragment}`);
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
      ".github/scripts/fixtures/workflow-policy-invalid.json",
      ".github/scripts/fixtures/actionlint-invalid.yml",
      ".github/scripts/run-actionlint.mjs",
      ".github/scripts/run-actionlint.test.mjs",
      ".github/actionlint.yaml",
      "release-claims.json",
      "scripts/codestory-release-claims.mjs",
      "scripts/codestory-release-evidence-gate.mjs",
      "scripts/tests/codestory-release-claims.test.mjs",
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
      "scripts/release-evidence/serde-json-codestory-project.json",
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
    ]);
    requireStepRun(violations, pluginFile, job, "Check plugin static wiring", ["node --test plugins/codestory/tests/plugin-static.test.mjs"]);
    requireStepRun(violations, pluginFile, job, "Check embedded model preparation", ["node --test scripts/tests/prepare-embedded-model.test.mjs"]);
    requireStepRun(violations, pluginFile, job, "Check workflow syntax", [
      "node --test .github/scripts/run-actionlint.test.mjs",
      "node .github/scripts/run-actionlint.mjs",
    ]);
    requireStepRun(violations, pluginFile, job, "Check release claim and evidence contracts", [
      "scripts/tests/codestory-release-claims.test.mjs",
      "scripts/tests/codestory-release-evidence-gate.test.mjs",
    ]);
    requireStepRun(violations, pluginFile, job, "Check CI proof routing fixtures", ["node .github/scripts/route-ci-proof.mjs --self-test"]);
    requireStepRun(violations, pluginFile, job, "Check packaged proof harness", ["python .github/scripts/check-packaged-agent-proof.py --self-test"]);
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
    requireResolverSemantics(violations, sourceFile, resolve, [
      'if [ -n "$EVENT_PR_NUMBER" ]; then',
      'test "$current_head" = "$EVENT_HEAD_SHA" || {',
      'if [ -n "$PR_NUMBER" ]; then',
      'test "$head_sha" = "$EXPECTED_HEAD_SHA" || {',
      'test "$GITHUB_REF" = "refs/heads/$head_ref" || {',
      'test "$GITHUB_SHA" = "$EXPECTED_HEAD_SHA" || {',
      'test "$GITHUB_SHA" = "$CALLER_REF" || {',
    ]);
    const full = requireJob(violations, sourceFile, source, "full-source-gate");
    add(violations, sameMembers(needs(full), ["resolve"]), `${sourceFile} full source gate must need resolve`);
    const restore = namedStep(full, "Restore Cargo inputs and output");
    const restoreWith = object(restore?.with);
    const expectedSourceKey = [
      "${{ runner.os }}-",
      promotion.source_cache_namespace,
      "-${{ needs.resolve.outputs.ref }}-${{ steps.rust-cache-key.outputs.version }}-",
      "${{ steps.rust-cache-key.outputs.target }}-workspace-all-targets-all-features-${{ hashFiles('Cargo.lock') }}",
    ].join("");
    add(
      violations,
      restore?.uses === "actions/cache/restore@v5"
        && restore?.["continue-on-error"] === true
        && restoreWith.key === expectedSourceKey,
      `${sourceFile} source cache must use the versioned exact-SHA namespace`,
    );
    add(
      violations,
      restoreWith["restore-keys"] === undefined,
      `${sourceFile} source cache must not use fallback restore keys`,
    );
    const save = namedStep(full, "Save Cargo inputs and output");
    add(
      violations,
      save?.uses === "actions/cache/save@v5"
        && save?.if === "success() && steps.cargo-cache-restore.outputs.cache-hit != 'true' && steps.cargo-cache-restore.outputs.cache-primary-key != ''"
        && object(save?.with).key === "${{ steps.cargo-cache-restore.outputs.cache-primary-key }}",
      `${sourceFile} source cache must save only a successful exact miss`,
    );
    const fullSteps = list(full.steps).map(object);
    add(
      violations,
      fullSteps.findIndex(step => step.name === "Save Cargo inputs and output") === fullSteps.length - 1,
      `${sourceFile} source cache save must run after every proof step`,
    );
    requireStepRun(violations, sourceFile, full, "Test the complete workspace once", ["cargo test --workspace --locked"]);
    requireStepRun(violations, sourceFile, full, "Lint every workspace target and feature once", ["cargo clippy --workspace --all-targets --all-features --locked -- -D warnings"]);
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
  add(violations, object(release.permissions).actions === "read", `${releaseFile} must read prior-run evidence`);
  for (const event of ["workflow_call", "workflow_dispatch"]) {
    const input = object(at(release, "on", event, "inputs", "source_run_id"));
    add(violations, input.required === false && input.type === "string" && input.default === "", `${releaseFile} ${event} source_run_id must be an optional empty string`);
    for (const key of ["calibration_bundle_artifact", "calibration_bundle_run_id"]) {
      const calibrationInput = object(at(release, "on", event, "inputs", key));
      add(
        violations,
        calibrationInput.required === true && calibrationInput.type === "string",
        `${releaseFile} ${event} ${key} must be a required string`,
      );
    }
  }
  const policy = requireJob(violations, releaseFile, release, "workflow-policy");
  requireStepRun(violations, releaseFile, policy, "Install workflow policy dependencies", ["npm ci --ignore-scripts"]);
  requireStepRun(violations, releaseFile, policy, "Check workflow syntax", [
    "node --test .github/scripts/run-actionlint.test.mjs",
    "node .github/scripts/run-actionlint.mjs",
  ]);
  requireStepRun(violations, releaseFile, policy, "Check release claim and evidence contracts", [
    "scripts/tests/codestory-release-claims.test.mjs",
    "scripts/tests/codestory-release-evidence-gate.test.mjs",
  ]);
  requireStepRun(violations, releaseFile, policy, "Enforce workflow policy", ["node .github/scripts/check-workflow-policy.mjs"]);

  const preflight = requireJob(violations, releaseFile, release, "preflight");
  add(violations, sameMembers(needs(preflight), releaseChain.dependencies.preflight), `${releaseFile} preflight dependencies must match the release claim graph`);
  requireStepRun(violations, releaseFile, preflight, "Validate versioned changelog notes", [
    "node .github/scripts/extract-codestory-release-notes.mjs",
    '--version "$VERSION"',
  ]);

  const evidence = requireJob(violations, releaseFile, release, "release-evidence");
  add(violations, sameMembers(needs(evidence), releaseChain.dependencies["release-evidence"]), `${releaseFile} release-evidence dependencies must match the release claim graph`);
  for (const [key, value] of Object.entries({
    ref: graph.workflow_policy.promotion.exact_sha_expression,
    proof_key: "release-${{ needs.preflight.outputs.version }}",
    profile: releaseChain.evidence_profile,
    drill_manifest: releaseChain.drill_manifest,
  })) {
    add(violations, object(evidence.with)[key] === value, `${releaseFile} release-evidence with.${key} must equal ${value}`);
  }

  const packaged = requireJob(violations, releaseFile, release, "packaged-proof");
  add(violations, packaged.uses === "./.github/workflows/packaged-platform-proof.yml", `${releaseFile} packaged-proof must call the package workflow`);
  add(violations, sameMembers(needs(packaged), releaseChain.dependencies["packaged-proof"]), `${releaseFile} packaged-proof dependencies must match the release claim graph`);
  add(violations, object(packaged.with).sign_macos === true, `${releaseFile} packaged-proof must sign Mac assets`);
  add(
    violations,
    object(packaged.with).candidate_installed_proof === undefined,
    `${releaseFile} must leave pre-merge candidate-installed proof to the PR coordinator`,
  );
  for (const job of [packaged]) {
    for (const key of ["calibration_bundle_artifact", "calibration_bundle_run_id"]) {
      add(
        violations,
        object(job.with)[key] === `\${{ inputs.${key} }}`,
        `${releaseFile} packaged proof must pass exact ${key}`,
      );
    }
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

  const metal = requireJob(violations, releaseFile, release, "macos-metal-proof");
  add(violations, metal.uses === "./.github/workflows/macos-metal-proof.yml", `${releaseFile} must call protected Metal proof`);
  add(violations, sameMembers(needs(metal), releaseChain.dependencies["macos-metal-proof"]), `${releaseFile} Metal proof dependencies must match the release claim graph`);
  add(violations, object(metal.with).use_packaged_cli_artifact === true, `${releaseFile} Metal proof must use the packaged CLI`);
  add(
    violations,
    object(metal.with).candidate_installed_proof === undefined
      && object(metal.with).server_behavior_only === undefined,
    `${releaseFile} must leave pre-merge candidate/server-only proof to the PR coordinator`,
  );

  const vulkan = requireJob(violations, releaseFile, release, "windows-vulkan-proof");
  add(violations, vulkan.uses === "./.github/workflows/windows-vulkan-proof.yml", `${releaseFile} must call protected Vulkan proof`);
  add(violations, sameMembers(needs(vulkan), releaseChain.dependencies["windows-vulkan-proof"]), `${releaseFile} Vulkan proof dependencies must match the release claim graph`);
  add(violations, object(vulkan.with).use_packaged_cli_artifact === true, `${releaseFile} Vulkan proof must use the packaged CLI`);

  const publish = requireJob(violations, releaseFile, release, "publish");
  add(violations, sameMembers(needs(publish), releaseChain.dependencies.publish), `${releaseFile} publish dependencies must match the release claim graph`);
  requireStepRun(violations, releaseFile, publish, "Compose versioned GitHub release notes", [
    "node .github/scripts/extract-codestory-release-notes.mjs",
    "--output target/release-assets/release-notes.md",
  ]);
  requireStepRun(violations, releaseFile, publish, "Create GitHub release", [
    "--notes-file target/release-assets/release-notes.md",
    'if [ "${#assets[@]}" -ne 7 ]; then',
  ]);
  add(violations, !scalarStrings(release).some(value => value.includes("--generate-notes")), `${releaseFile} must use curated release notes`);

  const post = requireJob(violations, releaseFile, release, "post-publish-smoke");
  add(violations, post.uses === "./.github/workflows/post-publish-release-smoke.yml", `${releaseFile} must call post-publish smoke`);
  add(violations, sameMembers(needs(post), releaseChain.dependencies["post-publish-smoke"]), `${releaseFile} post-publish dependencies must match the release claim graph`);
  for (const [jobName, job] of [
    ["Metal proof", metal],
    ["Vulkan proof", vulkan],
    ["post-publish proof", post],
  ]) {
    for (const key of ["calibration_bundle_artifact", "calibration_bundle_run_id"]) {
      add(
        violations,
        object(job.with)[key] === `\${{ inputs.${key} }}`,
        `${releaseFile} ${jobName} must pass exact ${key}`,
      );
    }
  }
}

function expectedPackageRows(graph) {
  return graph.workflow_policy.package_matrix;
}

function validatePackageMatrixExpression(violations, expression, graph) {
  const match = typeof expression === "string" && expression.match(
    /fromJSON\(inputs\.calibration_mode && '([^']+)' \|\| inputs\.scope == 'server' && '([^']+)' \|\| inputs\.scope == 'windows' && '([^']+)' \|\| inputs\.scope == 'macos' && '([^']+)' \|\| '([^']+)'\)/u,
  );
  if (!match) {
    violations.push("packaged-platform-proof.yml matrix must select structural JSON by scope");
    return;
  }
  const full = expectedPackageRows(graph);
  const expected = [
    { include: full.filter(row => row.asset_target === "linux-x64") },
    {
      include: full.filter(
        row => row.asset_target === "linux-x64" || row.asset_target === "macos-arm64",
      ),
    },
    { include: full.filter(row => row.asset_target === "windows-x64") },
    { include: full.filter(row => row.asset_target.startsWith("macos-")) },
    { include: full },
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
  add(violations, object(workflow.permissions).actions === "read", `${file} must read the prior-run calibration artifact`);
  for (const key of ["calibration_bundle_artifact", "calibration_bundle_run_id"]) {
    const input = object(at(workflow, "on", "workflow_call", "inputs", key));
    add(
      violations,
      input.required === false && input.type === "string" && input.default === "",
      `${file} ${key} must be an optional empty string until constants are frozen`,
    );
  }
  const candidateInput = object(at(
    workflow,
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
    `${file} candidate-installed proof must be an explicit opt-in`,
  );
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
    at(workflow, "concurrency", "group") === "packaged-platform-proof-${{ github.sha }}-${{ inputs.ref }}-${{ inputs.proof_key || github.ref }}",
    `${file} concurrency must bind caller SHA, exact package SHA, and proof identity`,
  );
  validatePackageMatrixExpression(violations, at(job, "strategy", "matrix"), graph);
  add(violations, String(job.environment ?? "").includes("macos-release-signing"), `${file} signed Mac cells must use the protected signing environment`);
  const packageSteps = list(job.steps).map(object);
  const nativeIdentitySteps = packageSteps.filter(step => step.name === "Capture Rust cache key");
  const nativeIdentity = nativeIdentitySteps[0];
  const nativeIdentityRun = executableRunText(String(nativeIdentity?.run ?? ""));
  add(
    violations,
    nativeIdentitySteps.length === 1
      && hasExactKeys(nativeIdentity, ["name", "id", "shell", "run"])
      && nativeIdentity?.id === "rust-cache-key"
      && nativeIdentity?.shell === "bash",
    `${file} native build identity must be unique, unconditional, and keep its exact Bash output boundary`,
  );
  for (const fragment of [
    'cmake=$(cmake --version',
    'if [ "$RUNNER_OS" = Windows ]',
    'generator=ninja',
    'ninja=$(ninja --version)',
    'CMAKE_GENERATOR=Ninja',
    'generator=platform-default',
    'ninja=not-applicable',
  ]) {
    add(
      violations,
      nativeIdentityRun.includes(fragment),
      `${file} native build identity must include ${fragment}`,
    );
  }
  const packageRestore = namedStep(job, "Restore Cargo registry, git sources, and build output");
  const installRustIndex = packageSteps.findIndex(step => step.name === "Install pinned Rust");
  const nativeIdentityIndex = packageSteps.findIndex(step => step.name === "Capture Rust cache key");
  const packageRestoreIndex = packageSteps.findIndex(step => step.name === "Restore Cargo registry, git sources, and build output");
  const packageBuildIndex = packageSteps.findIndex(step => step.name === "Build codestory-cli");
  const linuxBuildIndex = packageSteps.findIndex(step => step.name === "Build Linux x64 at the glibc 2.31 baseline");
  add(
    violations,
    nativeIdentityIndex === installRustIndex + 1
      && packageRestoreIndex === nativeIdentityIndex + 1
      && nativeIdentityIndex < packageBuildIndex
      && nativeIdentityIndex < linuxBuildIndex,
    `${file} native build identity must run immediately after Rust selection and before cache restore or any native build`,
  );
  const expectedPackageKey = [
    "${{ runner.os }}-release-${{ env.RELEASE_RUST_TOOLCHAIN }}-${{ steps.rust-cache-key.outputs.version }}-",
    "${{ matrix.rust_target }}-",
    graph.workflow_policy.promotion.packaged_cache_namespace,
    "-${{ inputs.ref }}-${{ steps.rust-cache-key.outputs.generator }}-cmake-${{ steps.rust-cache-key.outputs.cmake }}-",
    "ninja-${{ steps.rust-cache-key.outputs.ninja }}-default-features-",
    "${{ hashFiles('Cargo.lock', '.github/docker/linux-glibc-build.Dockerfile', '.github/docker/glslc') }}",
  ].join("");
  add(
    violations,
    object(packageRestore?.with).key === expectedPackageKey
      && object(packageRestore?.with)["restore-keys"] === undefined,
    `${file} native build cache must bind generator, CMake, Ninja, target, features, lock identity, and exact SHA/versioned namespace without fallbacks`,
  );
  const packageSave = namedStep(job, "Save Cargo registry, git sources, and build output");
  add(
    violations,
    packageSave?.uses === "actions/cache/save@v5"
      && packageSave?.if === "success() && steps.cargo-cache-restore.outputs.cache-hit != 'true' && steps.cargo-cache-restore.outputs.cache-primary-key != ''"
      && object(packageSave?.with).key === "${{ steps.cargo-cache-restore.outputs.cache-primary-key }}",
    `${file} native build cache must save only a successful exact miss`,
  );
  add(
    violations,
    packageSteps.findIndex(step => step.name === "Save Cargo registry, git sources, and build output") === packageSteps.length - 1,
    `${file} native build cache save must run after every proof and cleanup step`,
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
  requireStepRun(violations, file, job, "Prepare checksum-pinned embedded model", [
    "node scripts/prepare-embedded-model.mjs",
  ]);
  requireStepRun(violations, file, job, "Install Linux Vulkan build dependencies", [
    "bash .github/scripts/install-linux-vulkan-build-deps.sh",
  ]);
  requireStepRun(violations, file, job, "Build Linux x64 at the glibc 2.31 baseline", [
    ".github/docker/linux-glibc-build.Dockerfile",
    "cargo build --release --locked -p codestory-cli",
    "CARGO_TARGET_DIR=/workspace/target/glibc-2.31",
    "CXXFLAGS=-std=c++17",
  ]);
  requireStepRun(
    violations,
    file,
    job,
    "Prove clean-cache Node-absent network-denied offline release build",
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
  requireCalibrationProducerAuthentication(violations, file, job);
  requireStepUses(
    violations,
    file,
    job,
    "Download frozen calibration bundle",
    "actions/download-artifact@v8.0.1",
  );
  const calibrationDownload = namedStep(job, "Download frozen calibration bundle");
  add(
    violations,
    object(calibrationDownload?.with)["run-id"] === "${{ inputs.calibration_bundle_run_id }}"
      && object(calibrationDownload?.with).name === "${{ inputs.calibration_bundle_artifact }}"
      && object(calibrationDownload?.with)["github-token"] === "${{ github.token }}",
    `${file} frozen calibration download must bind its artifact name, prior run, and token`,
  );
  requireStepRun(
    violations,
    file,
    job,
    "Packaged per-user server calibration or qualification",
    [
      "proof_tier=hosted_package",
      "calibration-bundle.json",
      '--calibration-bundle "$calibration_bundle"',
      "--calibration-producer-run-id",
      "--calibration-producer-artifact",
      'if [ "$PROOF_SCOPE" = server ]',
      "--server-behavior-only",
      'test -f "$quality_path"',
      "--timeout-secs 1800",
    ],
  );
  const candidateStage = namedStep(job, "Stage isolated candidate-managed Linux install");
  add(
    violations,
    String(candidateStage?.if ?? "").includes("inputs.candidate_installed_proof")
      && String(candidateStage?.if ?? "").includes("inputs.scope == 'server'"),
    `${file} candidate-managed Linux staging must require coordinator opt-in and remain runnable in server scope`,
  );
  requireStepRun(violations, file, job, "Stage isolated candidate-managed Linux install", [
    "--prepare-candidate-installed-proof",
    "--candidate-plugin-root-output",
    "--candidate-plugin-data-output",
    "--installed-plugin-provenance-output",
    "--candidate-producer-workflow-path",
    "$RUNNER_TEMP/codestory-candidate-installed-linux.",
    'candidate_root="$(cd "$candidate_root" && pwd -P)"',
    '"$GITHUB_WORKSPACE/"*',
    "CODESTORY_CANDIDATE_LINUX_ROOT=",
  ]);
  const candidateProof = namedStep(job, "Prove two-host candidate-installed Linux runtime");
  add(
    violations,
    String(candidateProof?.if ?? "").includes("inputs.candidate_installed_proof")
      && String(candidateProof?.if ?? "").includes("inputs.scope == 'server'"),
    `${file} candidate-installed Linux proof must require coordinator opt-in and remain runnable in server scope`,
  );
  requireStepRun(violations, file, job, "Prove two-host candidate-installed Linux runtime", [
    "--proof-tier installed_runtime",
    "--qualification-matrix-cell candidate_installed_linux_x64_cpu",
    "--installed-plugin-source candidate",
    "--installed-plugin-provenance",
    "--installed-plugin-data",
    "--calibration-producer-run-id",
    "--calibration-producer-artifact",
    "--server-behavior-only",
    "$CODESTORY_CANDIDATE_LINUX_ROOT/plugin",
    "$CODESTORY_CANDIDATE_LINUX_ROOT/data",
    'test -f "$quality_path"',
  ]);
  violations.push(...managedPluginViolations(
    job,
    '--archive "target/release-dist/codestory-cli-v${{ inputs.version }}-${{ matrix.asset_target }}.${{ matrix.extension }}"',
  ).map(message => `${file} ${message}`));
  requireStepUses(violations, file, job, "Upload release asset", "actions/upload-artifact@v7.0.1");
  requireStepUses(violations, file, job, "Upload macOS notarization proof", "actions/upload-artifact@v7.0.1");
}

function validatePostPublish(workflows, violations, graph) {
  const file = "post-publish-release-smoke.yml";
  const workflow = workflows.get(file);
  if (!workflow) {
    violations.push(`${file} must exist`);
    return;
  }
  add(violations, trigger(workflow, "workflow_call") !== undefined, `${file} must be reusable`);
  add(violations, object(workflow.permissions).actions === "read", `${file} must read the prior-run calibration artifact`);
  for (const event of ["workflow_call", "workflow_dispatch"]) {
    for (const key of ["calibration_bundle_artifact", "calibration_bundle_run_id"]) {
      const input = object(at(workflow, "on", event, "inputs", key));
      add(
        violations,
        input.required === true && input.type === "string",
        `${file} ${event} ${key} must be a required string`,
      );
    }
  }
  const job = requireJob(violations, file, workflow, "smoke");
  const expected = expectedPackageRows(graph).map(({ os, asset_target, extension }) => ({ os, asset_target, extension }));
  add(violations, JSON.stringify(at(job, "strategy", "matrix", "include")) === JSON.stringify(expected), `${file} must smoke all six native assets`);
  const resolveInstalled = namedStep(job, "Resolve the marketplace-installed plugin");
  requireStepRun(violations, file, job, "Resolve the marketplace-installed plugin", [
    "TheGreenCedar/AgentPluginMarketplace",
    "refs/heads/main",
    "Marketplace source main is not the exact published release commit",
    "plugin_source_commit",
    "plugin_package_sha256",
  ]);
  add(
    violations,
    resolveInstalled?.if === undefined
      && resolveInstalled?.["continue-on-error"] === undefined,
    `${file} installed plugin resolution must be unconditional and fail closed`,
  );
  const installed = namedStep(job, "Qualify the marketplace-managed installed runtime");
  add(violations, installed !== undefined, `${file} installed runtime proof step is missing`);
  add(
    violations,
    installed?.if === undefined
      && installed?.["continue-on-error"] === undefined,
    `${file} installed runtime proof must be unconditional and fail closed`,
  );
  add(
    violations,
    object(installed?.env).CODESTORY_EMBED_ALLOW_CPU === "1",
    `${file} installed runtime proof must explicitly allow CPU evidence`,
  );
  const installedRun = executableRunText(String(installed?.run ?? ""));
  for (const fragment of [
    "python .github/scripts/check-packaged-agent-proof.py",
    '--archive "${{ steps.asset.outputs.archive }}"',
    "--plugin-handoff",
    "--engine-policy cpu_explicit",
    "--expected-backend CPU",
    "--proof-tier installed_runtime",
    "--produce-qualification-evidence",
    "--installed-plugin-provenance",
    "--installed-plugin-data",
    "--expected-source-sha",
    "--expected-source-tree",
    '--calibration-bundle "$calibration_bundle"',
    "--calibration-producer-run-id",
    "--calibration-producer-artifact",
    "--installed-plugin-source marketplace",
  ]) {
    add(
      violations,
      installedRun.includes(fragment),
      `${file} installed runtime proof must run ${fragment}`,
    );
  }
  requireCalibrationProducerAuthentication(violations, file, job);
  requireStepUses(
    violations,
    file,
    job,
    "Download frozen calibration bundle",
    "actions/download-artifact@v8.0.1",
  );
  const calibrationDownload = namedStep(job, "Download frozen calibration bundle");
  add(
    violations,
    object(calibrationDownload?.with)["run-id"] === "${{ inputs.calibration_bundle_run_id }}"
      && object(calibrationDownload?.with).name === "${{ inputs.calibration_bundle_artifact }}"
      && object(calibrationDownload?.with)["github-token"] === "${{ github.token }}",
    `${file} frozen calibration download must bind its artifact name, prior run, and token`,
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
      ["platform", "calibration", "release-evidence", "integration"],
    ),
    `${file} dispatch modes changed`,
  );
  add(
    violations,
    sameMembers(
      at(workflow, "on", "workflow_dispatch", "inputs", "scope", "options"),
      ["auto", "server", "windows", "macos", "full"],
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
  requireResolverSemantics(violations, file, route, [
    'if [ "$mode" = "integration" ]; then',
    'test "$INPUT_HEAD_SHA" = "$dev_head" || {',
    'test "$GITHUB_REF" = "refs/heads/dev/codestory-next" || {',
    'test "$GITHUB_SHA" = "$dev_head" || {',
    'if [ -n "$EVENT_HEAD_REPO" ]; then',
    'test "$current_head" = "$EVENT_HEAD_SHA" || {',
    'test "$INPUT_HEAD_SHA" = "$current_head" || {',
    'test "$GITHUB_REF" = "refs/heads/$head_ref" || {',
    'test "$GITHUB_SHA" = "$INPUT_HEAD_SHA" || {',
  ]);
  requireStepRun(violations, file, route, "Require successful exact-head source proof", [
    "actions/runs?head_sha=$HEAD_SHA",
    '.path == ".github/workflows/source-proof.yml"',
    '.name == "full-source-gate" and .conclusion == "success"',
  ]);
  requireStepRun(violations, file, route, "Select change-aware proof scope", ["node .github/scripts/route-ci-proof.mjs --stdin"]);
  requireStepRun(violations, file, route, "Read qualification constant-set state", [
    "base_frozen=false",
    "jq -r '.status == \"frozen\"'",
    'git cat-file -e "$BASE_SHA:$constant_set_path"',
    'test "$frozen" = true || test "$frozen" = false',
    'test "$base_frozen" = true || test "$base_frozen" = false',
    'freeze_transition=false\nif [ -n "$BASE_SHA" ] \\\n  && [ "$base_frozen" = false ] \\\n  && [ "$frozen" = true ]; then',
  ]);
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
      && object(calibrationLinux.with).calibration_mode === true,
    `${file} hosted Linux calibration must call packaged proof in calibration mode`,
  );
  const calibrationMacos = requireJob(violations, file, workflow, "calibration-macos");
  add(
    violations,
    calibrationMacos.uses === "./.github/workflows/macos-metal-proof.yml"
      && object(calibrationMacos.with).calibration_mode === true,
    `${file} protected macOS calibration must call Metal proof in calibration mode`,
  );
  const macosSource = requireJob(violations, file, workflow, "macos-source");
  const macosRestore = namedStep(macosSource, "Restore exact-head macOS source cache");
  const expectedMacosKey = [
    "${{ runner.os }}-",
    promotion.macos_source_cache_namespace,
    "-${{ needs.route.outputs.head_sha }}-${{ steps.rust-cache-key.outputs.version }}-",
    "${{ matrix.target }}-workspace-default-features-${{ hashFiles('Cargo.lock') }}",
  ].join("");
  add(
    violations,
    macosRestore?.uses === "actions/cache/restore@v5"
      && macosRestore?.["continue-on-error"] === true
      && object(macosRestore?.with).key === expectedMacosKey
      && object(macosRestore?.with)["restore-keys"] === undefined,
    `${file} macOS source cache must be an exact-SHA restore without fallbacks`,
  );
  const macosSave = namedStep(macosSource, "Save exact-head macOS source cache");
  add(
    violations,
    macosSave?.uses === "actions/cache/save@v5"
      && macosSave?.if === "success() && steps.macos-source-cache-restore.outputs.cache-hit != 'true' && steps.macos-source-cache-restore.outputs.cache-primary-key != ''"
      && object(macosSave?.with).key === "${{ steps.macos-source-cache-restore.outputs.cache-primary-key }}",
    `${file} macOS source cache must save only a successful exact miss`,
  );
  const macosSourceSteps = list(macosSource.steps).map(object);
  add(
    violations,
    macosSourceSteps.findIndex(step => step.name === "Save exact-head macOS source cache") === macosSourceSteps.length - 1,
    `${file} macOS source cache save must run after every proof step`,
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
    object(packaged.with).enforce_calibration_freeze_lineage
      === "${{ needs.route.outputs.freeze_transition == 'true' }}",
    `${file} must enforce calibration lineage only on the detected freeze transition`,
  );
  add(
    violations,
    object(packaged.with).candidate_installed_proof === true,
    `${file} must opt the accepted PR package into candidate-installed proof`,
  );
  violations.push(...packagedPrSigningViolations(workflow));
  const metal = requireJob(violations, file, workflow, "macos-metal-proof");
  add(
    violations,
    sameMembers(needs(metal), ["route", "release-evidence", "packaged-proof"]),
    `${file} Metal proof must wait for package and exact-head release evidence`,
  );
  add(violations, object(metal.with).use_packaged_cli_artifact === true, `${file} Metal proof must use the packaged CLI`);
  add(
    violations,
    object(metal.with).candidate_installed_proof === true,
    `${file} must opt the accepted PR Metal package into candidate-installed proof`,
  );
  add(
    violations,
    object(metal.with).server_behavior_only
      === "${{ needs.route.outputs.scope == 'server' }}",
    `${file} must select the non-quality server claim only for server scope`,
  );
  const vulkan = requireJob(violations, file, workflow, "windows-vulkan-proof");
  add(
    violations,
    sameMembers(needs(vulkan), ["route", "release-evidence", "packaged-proof"]),
    `${file} Vulkan proof must wait for package and exact-head release evidence`,
  );
  add(violations, object(vulkan.with).use_packaged_cli_artifact === true, `${file} Vulkan proof must use the packaged CLI`);
  const closeout = requireJob(violations, file, workflow, "closeout");
  requireStepRun(violations, file, closeout, "Require one coherent accepted proof", ["dev/codestory-next moved from proved head"]);
  add(violations, !scalarStrings(workflow).some(value => value === "./.github/workflows/release.yml"), `${file} must not publish releases`);
}

function validateRemainingWorkflows(workflows, violations) {
  const autoFile = "auto-release.yml";
  const auto = workflows.get(autoFile);
  if (!auto) {
    violations.push(`${autoFile} must exist`);
  } else {
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
    const policy = requireJob(violations, autoFile, auto, "workflow-policy");
    requireStepRun(violations, autoFile, policy, "Install workflow policy dependencies", ["npm ci --ignore-scripts"]);
    requireStepRun(violations, autoFile, policy, "Check workflow syntax", [
      "node --test .github/scripts/run-actionlint.test.mjs",
      "node .github/scripts/run-actionlint.mjs",
    ]);
    requireStepRun(violations, autoFile, policy, "Check release claim and evidence contracts", [
      "scripts/tests/codestory-release-claims.test.mjs",
      "scripts/tests/codestory-release-evidence-gate.test.mjs",
    ]);
    requireStepRun(violations, autoFile, policy, "Enforce workflow policy", ["node .github/scripts/check-workflow-policy.mjs"]);
    const release = requireJob(violations, autoFile, auto, "release");
    add(violations, release.uses === "./.github/workflows/release.yml", `${autoFile} must call the release workflow`);
    add(violations, sameMembers(needs(release), ["detect-version"]), `${autoFile} release must need version detection`);
    add(violations, object(release.permissions).contents === "write", `${autoFile} release caller must grant contents write`);
    add(violations, object(release.permissions).actions === "read", `${autoFile} release caller must grant actions read`);
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
        ["release.yml", at(workflows.get("release.yml"), "jobs", "release-evidence"), true],
        ["packaged-platform-pr.yml", at(workflows.get("packaged-platform-pr.yml"), "jobs", "release-evidence"), false],
      ],
      evidence,
    ));
    requireStepRun(violations, evidenceFile, job, "Produce full-retrieval repo evidence", ["--test-threads=1"]);
    requireStepRun(violations, evidenceFile, job, "Download prior rejected evidence for approval re-evaluation", ["actions/runs/$SOURCE_RUN_ID", "actions/runs/$SOURCE_RUN_ID/artifacts"]);
  }

  const metalFile = "macos-metal-proof.yml";
  const metal = workflows.get(metalFile);
  if (!metal) {
    violations.push(`${metalFile} must exist`);
  } else {
    add(violations, trigger(metal, "workflow_call") !== undefined && trigger(metal, "workflow_dispatch") !== undefined, `${metalFile} must support reusable and manual proof`);
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
    requireStepRun(violations, metalFile, job, "Prepare checksum-pinned embedded model", ["node scripts/prepare-embedded-model.mjs"]);
    requireStepRun(violations, metalFile, job, "Capture host evidence", ["python3 --version", 'test "$macos_major" -ge 15']);
    requireCalibrationProducerAuthentication(violations, metalFile, job);
    const engine = namedStep(job, "Prove cold and warm Metal, offline packaging, and multi-repository reuse");
    requireStepRun(violations, metalFile, job, "Prove cold and warm Metal, offline packaging, and multi-repository reuse", [
      "--engine-policy accelerated",
      "--expected-backend Metal",
      "--offline",
      "--calibration-producer-run-id",
      "--calibration-producer-artifact",
      "--server-behavior-only",
      'test -f "$quality_path"',
    ]);
    add(violations, object(engine?.env).CODESTORY_EMBED_ALLOW_CPU === "0", `${metalFile} engine proof must reject CPU fallback`);
    const candidateStage = namedStep(job, "Stage isolated candidate-managed macOS install");
    add(
      violations,
      String(candidateStage?.if ?? "").includes("inputs.candidate_installed_proof")
        && String(candidateStage?.if ?? "").includes("inputs.server_behavior_only"),
      `${metalFile} candidate-managed staging must require coordinator opt-in and remain runnable in server scope`,
    );
    requireStepRun(violations, metalFile, job, "Stage isolated candidate-managed macOS install", [
      "--prepare-candidate-installed-proof",
      "--candidate-plugin-root-output",
      "--candidate-plugin-data-output",
      "--installed-plugin-provenance-output",
      "--candidate-producer-workflow-path",
      "$RUNNER_TEMP/codestory-candidate-installed-macos.",
      'candidate_root="$(cd "$candidate_root" && pwd -P)"',
      '"$GITHUB_WORKSPACE/"*',
      "CODESTORY_CANDIDATE_MACOS_ROOT=",
    ]);
    const candidateProof = namedStep(job, "Prove two-host candidate-installed macOS runtime");
    add(
      violations,
      String(candidateProof?.if ?? "").includes("inputs.candidate_installed_proof")
        && String(candidateProof?.if ?? "").includes("inputs.server_behavior_only"),
      `${metalFile} candidate-installed proof must require coordinator opt-in and remain runnable in server scope`,
    );
    requireStepRun(violations, metalFile, job, "Prove two-host candidate-installed macOS runtime", [
      "--proof-tier installed_runtime",
      "--qualification-matrix-cell candidate_installed_macos_arm64_cpu",
      "--installed-plugin-source candidate",
      "--installed-plugin-provenance",
      "--installed-plugin-data",
      "--calibration-producer-run-id",
      "--calibration-producer-artifact",
      "--server-behavior-only",
      "$CODESTORY_CANDIDATE_MACOS_ROOT/plugin",
      "$CODESTORY_CANDIDATE_MACOS_ROOT/data",
      'test -f "$quality_path"',
    ]);
  }

  const vulkanFile = "windows-vulkan-proof.yml";
  const vulkan = workflows.get(vulkanFile);
  if (!vulkan) {
    violations.push(`${vulkanFile} must exist`);
  } else {
    add(violations, trigger(vulkan, "workflow_call") !== undefined && trigger(vulkan, "workflow_dispatch") !== undefined, `${vulkanFile} must support reusable and manual proof`);
    const job = requireJob(violations, vulkanFile, vulkan, "packaged-vulkan");
    add(violations, JSON.stringify(job["runs-on"]) === JSON.stringify(["self-hosted", "Windows", "X64", "codestory-vulkan"]), `${vulkanFile} must use the protected Windows Vulkan runner`);
    add(violations, job.environment === "windows-vulkan-proof", `${vulkanFile} must use the protected Vulkan environment`);
    const sourceBuildTools = namedStep(job, "Capture source build tool evidence");
    add(
      violations,
      hasExactKeys(object(sourceBuildTools), ["name", "if", "shell", "run"])
        && sourceBuildTools?.if === "${{ !inputs.use_packaged_cli_artifact }}"
        && sourceBuildTools?.shell === "pwsh",
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
    requireCalibrationProducerAuthentication(violations, vulkanFile, job);
    const engine = namedStep(job, "Prove offline Vulkan and multi-repository reuse");
    requireStepRun(violations, vulkanFile, job, "Prove offline Vulkan and multi-repository reuse", [
      "--engine-policy accelerated",
      "--expected-backend Vulkan",
      "--offline",
      "--calibration-producer-run-id",
      "--calibration-producer-artifact",
    ]);
    add(violations, object(engine?.env).CODESTORY_EMBED_ALLOW_CPU === "0", `${vulkanFile} engine proof must reject CPU fallback`);
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
  const releaseEvidence = object(at(release, "jobs", "release-evidence"));
  add(
    violations,
    releaseEvidence.uses === policy.release_chain.evidence_workflow,
    `[proof_contract] release.yml must call ${policy.release_chain.evidence_workflow}`,
  );
  add(
    violations,
    object(releaseEvidence.with).profile === policy.release_chain.evidence_profile
      && object(releaseEvidence.with).drill_manifest === policy.release_chain.drill_manifest,
    "[proof_contract] release.yml evidence profile and drill must match the release claim graph",
  );

  const matrixViolations = [];
  validatePackageMatrixExpression(
    matrixViolations,
    at(workflows.get("packaged-platform-proof.yml"), "jobs", "build", "strategy", "matrix"),
    graph,
  );
  const smokeMatrix = at(workflows.get("post-publish-release-smoke.yml"), "jobs", "smoke", "strategy", "matrix", "include");
  const expectedSmoke = expectedPackageRows(graph).map(({ os, asset_target, extension }) => ({ os, asset_target, extension }));
  add(matrixViolations, JSON.stringify(smokeMatrix) === JSON.stringify(expectedSmoke), "post-publish package target matrix changed");
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

export function validateWorkflows(workflows, graph = loadReleaseClaimGraph(repositoryRoot)) {
  const violations = [];
  for (const [file, workflow] of workflows) {
    violations.push(...basicWorkflowViolations(file, workflow));
  }
  validateLockedSetupSurfaces(violations);
  validateIssueWorkflows(workflows, violations);
  validatePluginAndDraftWorkflows(workflows, violations, graph);
  validateReleaseCoordinator(workflows, violations, graph);
  validatePackagedProof(workflows, violations, graph);
  validatePostPublish(workflows, violations, graph);
  validatePackagedCoordinator(workflows, violations, graph);
  validateRemainingWorkflows(workflows, violations);
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
