#!/usr/bin/env node
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { LineCounter, parseDocument } from "yaml";
import { loadReleaseClaimGraph } from "../../scripts/codestory-release-claims.mjs";

const workflowRoot = path.join(".github", "workflows");
const repositoryRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "../..");
const trustedActionOwners = new Set(["actions", "github"]);
const fullSha = /^[0-9a-f]{40}$/iu;

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

function requireStepUses(violations, file, job, name, expected) {
  add(
    violations,
    namedStep(job, name)?.uses === expected,
    `${file} step ${name} must use ${expected}`,
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
const draftCachePrimary = "${{ runner.os }}-draft-v2-${{ steps.rust-cache-key.outputs.version }}-${{ steps.rust-cache-key.outputs.target }}-workspace-default-features-${{ hashFiles('Cargo.lock', 'Cargo.toml', 'crates/**/Cargo.toml', 'vendor/**/Cargo.toml') }}";
const draftCacheRestoreKeys = [
  "${{ runner.os }}-cargo-stable-${{ steps.rust-cache-key.outputs.version }}-${{ steps.rust-cache-key.outputs.target }}-retrieval-contracts-default-features-${{ hashFiles('Cargo.lock') }}",
  "${{ runner.os }}-draft-v2-${{ steps.rust-cache-key.outputs.version }}-${{ steps.rust-cache-key.outputs.target }}-workspace-default-features-",
  "${{ runner.os }}-cargo-stable-${{ steps.rust-cache-key.outputs.version }}-${{ steps.rust-cache-key.outputs.target }}-retrieval-contracts-default-features-",
];
const draftStepSequence = [
  { uses: "actions/checkout@v5" },
  { name: "Install Rust stable" },
  { name: "Install Linux Vulkan build dependencies" },
  { name: "Capture Rust cache identity" },
  { name: "Restore Cargo inputs and output" },
  { name: "Check formatting" },
  { name: "Check immutable runtime configuration boundary" },
  { name: "Check the workspace" },
  { name: "Check generated CodeStory syntax and MCP catalog" },
  { name: "Lint workspace libraries" },
  { name: "Prove focused publication contracts" },
  { name: "Save Cargo inputs and output" },
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
  ["Prove focused publication contracts", [
    "cargo test --locked -p codestory-llama-sys --test model_staging",
    "cargo test --locked -p codestory-cli --test stdio_protocol_contracts two_stdio_processes_observe_only_complete_generations_during_real_refresh -- --nocapture",
    "cargo test --locked -p codestory-runtime publication_transitions_fail_or_cancel_atomically -- --nocapture",
    "cargo test --locked -p codestory-store staged_promotion_abort_recovers_old_or_complete_new_and_cleans_artifacts -- --nocapture",
  ]],
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

export function draftSourcePolicyViolations(jobValue, retrievalJobValue) {
  const violations = [];
  const job = object(jobValue);
  const retrievalJob = object(retrievalJobValue);
  const steps = list(job.steps).map(object);

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
  add(violations, sameStrings(nonCommentLines(restoreWith.path), draftCachePaths), "draft source cache restore must use only the Cargo registry, git, and default target paths");
  add(violations, restoreWith.key === draftCachePrimary, "draft source cache primary must bind the v2 platform, toolchain, feature, lock, and manifest identity");
  add(violations, sameStrings(nonCommentLines(restoreWith["restore-keys"]), draftCacheRestoreKeys), "draft source cache fallbacks must keep the exact retrieval, prior draft, then prior retrieval order");

  const retrievalRestore = namedStep(retrievalJob, "Restore Cargo registry, git sources, and build output");
  const retrievalRestoreWith = object(retrievalRestore?.with);
  add(violations, retrievalRestore?.id === "cargo-cache-restore", "retrieval cache producer must keep its stable restore id");
  add(violations, retrievalRestore?.uses === "actions/cache/restore@v5", "retrieval cache producer must use actions/cache/restore@v5");
  add(violations, retrievalRestore?.["continue-on-error"] === true, "retrieval cache producer restore must remain non-blocking");
  add(violations, sameStrings(nonCommentLines(retrievalRestoreWith.path), draftCachePaths), "retrieval cache producer must retain the proof-compatible path set");
  add(violations, retrievalRestoreWith.key === draftCacheRestoreKeys[0], "retrieval cache producer key must match the draft exact-lock fallback");

  const save = namedStep(job, "Save Cargo inputs and output");
  const saveWith = object(save?.with);
  add(violations, save?.uses === "actions/cache/save@v5", "draft source cache promotion must use actions/cache/save@v5");
  add(violations, save?.["continue-on-error"] === true, "draft source cache promotion must remain non-blocking");
  add(
    violations,
    save?.if === "success() && steps.cargo-cache-restore.outputs.cache-hit != 'true' && steps.cargo-cache-restore.outputs.cache-primary-key != ''",
    "draft source cache promotion must require complete proof and a partial or missing primary",
  );
  add(violations, sameStrings(nonCommentLines(saveWith.path), draftCachePaths), "draft source cache promotion must use the exact restore path set");
  add(violations, saveWith.key === "${{ steps.cargo-cache-restore.outputs.cache-primary-key }}", "draft source cache promotion must save the exact primary rather than a matched fallback");

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

function validatePluginAndDraftWorkflows(workflows, violations) {
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
    const retrievalJob = object(at(
      workflows.get("retrieval-engine-smoke.yml"),
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
    add(violations, sameMembers(at(source, "on", "pull_request", "types"), ["labeled", "synchronize"]), `${sourceFile} pull request types must be labeled and synchronize`);
    add(violations, trigger(source, "pull_request_target") === undefined, `${sourceFile} must not execute pull-request code through pull_request_target`);
    const resolve = requireJob(violations, sourceFile, source, "resolve");
    add(violations, String(resolve.if ?? "").includes("review-accepted"), `${sourceFile} resolve job must require review-accepted`);
    requireStepRun(violations, sourceFile, resolve, "Resolve trusted exact head", [
      'test "$EVENT_HEAD_REPO" = "$GITHUB_REPOSITORY"',
      'test "$current_head" = "$EVENT_HEAD_SHA"',
    ]);
    const full = requireJob(violations, sourceFile, source, "full-source-gate");
    add(violations, sameMembers(needs(full), ["resolve"]), `${sourceFile} full source gate must need resolve`);
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
}

function expectedPackageRows(graph) {
  return graph.workflow_policy.package_matrix;
}

function validatePackageMatrixExpression(violations, expression, graph) {
  const match = typeof expression === "string" && expression.match(
    /fromJSON\(inputs\.scope == 'windows' && '([^']+)' \|\| inputs\.scope == 'macos' && '([^']+)' \|\| '([^']+)'\)/u,
  );
  if (!match) {
    violations.push("packaged-platform-proof.yml matrix must select structural JSON by scope");
    return;
  }
  const full = expectedPackageRows(graph);
  const expected = [
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
  add(violations, object(workflow.permissions).contents === "read", `${file} must use read-only contents permission`);
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
  validatePackageMatrixExpression(violations, at(job, "strategy", "matrix"), graph);
  add(violations, String(job.environment ?? "").includes("macos-release-signing"), `${file} signed Mac cells must use the protected signing environment`);
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
  const job = requireJob(violations, file, workflow, "smoke");
  const expected = expectedPackageRows(graph).map(({ os, asset_target, extension }) => ({ os, asset_target, extension }));
  add(violations, JSON.stringify(at(job, "strategy", "matrix", "include")) === JSON.stringify(expected), `${file} must smoke all six native assets`);
  violations.push(...managedPluginViolations(
    job,
    '--archive "${{ steps.asset.outputs.archive }}"',
  ).map(message => `${file} ${message}`));
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
  requireStepRun(violations, file, job, "Prove published Intel macOS explicit CPU policy", ["--engine-policy cpu_explicit", "--expected-backend CPU", "--offline"]);
  add(violations, !scalarStrings(workflow).some(value => value.includes("sha256sum")), `${file} must use the portable Python checksum gate`);
}

function validatePackagedCoordinator(workflows, violations) {
  const file = "packaged-platform-pr.yml";
  const workflow = workflows.get(file);
  if (!workflow) {
    violations.push(`${file} must exist`);
    return;
  }
  add(violations, sameMembers(at(workflow, "on", "pull_request", "types"), ["labeled", "synchronize"]), `${file} pull request types must be labeled and synchronize`);
  add(violations, includesAll(at(workflow, "on", "workflow_dispatch", "inputs", "mode", "options"), ["platform", "release-evidence", "integration"]), `${file} dispatch modes changed`);
  add(violations, includesAll(at(workflow, "on", "workflow_dispatch", "inputs", "scope", "options"), ["auto", "windows", "macos", "full"]), `${file} dispatch scopes changed`);
  add(violations, trigger(workflow, "pull_request_target") === undefined, `${file} must not use pull_request_target`);
  add(violations, object(workflow.permissions).actions === "read", `${file} must read source-proof runs`);
  add(violations, object(workflow.permissions).contents === "read", `${file} must use read-only contents permission`);
  const route = requireJob(violations, file, workflow, "route");
  requireStepRun(violations, file, route, "Resolve trusted exact head", [
    'test "$head_repo" = "$GITHUB_REPOSITORY"',
    'test "$current_head" = "$expected_head"',
    'test "$base_ref" = "dev/codestory-next"',
  ]);
  requireStepRun(violations, file, route, "Require successful exact-head source proof", [
    "actions/runs?head_sha=$HEAD_SHA",
    '.path == ".github/workflows/source-proof.yml"',
    '.name == "full-source-gate" and .conclusion == "success"',
  ]);
  requireStepRun(violations, file, route, "Select change-aware proof scope", ["node .github/scripts/route-ci-proof.mjs --stdin"]);
  const packaged = requireJob(violations, file, workflow, "packaged-proof");
  add(violations, packaged.uses === "./.github/workflows/packaged-platform-proof.yml", `${file} must call packaged proof`);
  violations.push(...packagedPrSigningViolations(workflow));
  const metal = requireJob(violations, file, workflow, "macos-metal-proof");
  add(violations, sameMembers(needs(metal), ["route", "packaged-proof"]), `${file} Metal proof must wait for package proof`);
  add(violations, object(metal.with).use_packaged_cli_artifact === true, `${file} Metal proof must use the packaged CLI`);
  const vulkan = requireJob(violations, file, workflow, "windows-vulkan-proof");
  add(violations, sameMembers(needs(vulkan), ["route", "packaged-proof"]), `${file} Vulkan proof must wait for package proof`);
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
    const job = requireJob(violations, metalFile, metal, "packaged-metal");
    add(violations, JSON.stringify(job["runs-on"]) === JSON.stringify(["self-hosted", "macOS", "ARM64", "codestory-metal"]), `${metalFile} must use the protected Apple Silicon runner`);
    add(violations, job.environment === "macos-metal-release", `${metalFile} must use the protected Metal environment`);
    requireStepRun(violations, metalFile, job, "Prepare checksum-pinned embedded model", ["node scripts/prepare-embedded-model.mjs"]);
    requireStepRun(violations, metalFile, job, "Capture host evidence", ["python3 --version", 'test "$macos_major" -ge 15']);
    const engine = namedStep(job, "Prove cold and warm Metal, offline packaging, and multi-repository reuse");
    requireStepRun(violations, metalFile, job, "Prove cold and warm Metal, offline packaging, and multi-repository reuse", ["--engine-policy accelerated", "--expected-backend Metal", "--offline"]);
    add(violations, object(engine?.env).CODESTORY_EMBED_ALLOW_CPU === "0", `${metalFile} engine proof must reject CPU fallback`);
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
    requireStepRun(violations, vulkanFile, job, "Prepare checksum-pinned embedded model", ["node scripts/prepare-embedded-model.mjs"]);
    const engine = namedStep(job, "Prove offline Vulkan and multi-repository reuse");
    requireStepRun(violations, vulkanFile, job, "Prove offline Vulkan and multi-repository reuse", ["--engine-policy accelerated", "--expected-backend Vulkan", "--offline"]);
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

  const retrievalFile = "retrieval-engine-smoke.yml";
  const retrieval = workflows.get(retrievalFile);
  if (!retrieval) {
    violations.push(`${retrievalFile} must exist`);
  } else {
    const windows = requireJob(violations, retrievalFile, retrieval, "windows-manifest-missing");
    add(violations, windows.if === "github.event_name == 'workflow_dispatch'", `${retrievalFile} Windows proof must be workflow-dispatch only`);
    add(violations, !scalarStrings(windows).some(value => value.includes("labels")), `${retrievalFile} Windows proof must not be label-triggered`);
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
      `[persistent_label] ${file} must re-evaluate labels on ${policy.promotion.required_events.join(" and ")}`,
    );
    const resolver = findNamedStep(workflow, "Resolve trusted exact head");
    const run = executableRunText(String(resolver?.run ?? ""));
    add(
      violations,
      run.includes("current_head") && (run.includes("EVENT_HEAD_SHA") || run.includes("expected_head")),
      `[persistent_label] ${file} must resolve the current head and compare its exact SHA before executing labeled work`,
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
  validatePluginAndDraftWorkflows(workflows, violations);
  validateReleaseCoordinator(workflows, violations, graph);
  validatePackagedProof(workflows, violations, graph);
  validatePostPublish(workflows, violations, graph);
  validatePackagedCoordinator(workflows, violations);
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
