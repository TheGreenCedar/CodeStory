#!/usr/bin/env node
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { LineCounter, parseDocument } from "yaml";

const workflowRoot = path.join(".github", "workflows");
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

export const releaseEvidenceWorkflowRef = "./.github/workflows/release-candidate-evidence.yml";

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

function loadWorkflows(root = workflowRoot) {
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
  const run = executableRunText(typeof step?.run === "string" ? step.run : "");
  for (const fragment of [
    "python .github/scripts/check-packaged-agent-proof.py",
    archiveFragment,
    "--managed-plugin-handoff",
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
    [path.join("scripts", "codex-worktree-setup.mjs"), ['["build", "--release", "--locked", "-p", "codestory-cli"]']],
    [path.join("plugins", "codestory", "skills", "codestory-grounding", "scripts", "setup.sh"), ["cargo build --release --locked -p codestory-cli"]],
    [path.join("plugins", "codestory", "skills", "codestory-grounding", "scripts", "setup.ps1"), ['@("build", "--release", "--locked", "-p", "codestory-cli"']],
    [path.join("scripts", "setup-retrieval-env.mjs"), ['return ["run", "--locked", ...args]']],
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
      ".github/workflows/release.yml",
      ".github/workflows/packaged-platform-pr.yml",
      ".github/workflows/packaged-platform-proof.yml",
      ".github/workflows/macos-metal-proof.yml",
      ".github/workflows/source-proof.yml",
      ".github/workflows/repo-scale-stats.yml",
      "package.json",
      "package-lock.json",
      "scripts/codex-worktree-setup.*",
      "scripts/setup-retrieval-env.*",
      "scripts/install-codestory.ps1",
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
    requireStepRun(violations, pluginFile, job, "Check CI proof routing fixtures", ["node .github/scripts/route-ci-proof.mjs --self-test"]);
    requireStepRun(violations, pluginFile, job, "Check packaged proof harness", ["python .github/scripts/check-packaged-agent-proof.py --self-test"]);
  }

  const rustFile = "rust-ci.yml";
  const rust = workflows.get(rustFile);
  if (!rust) {
    violations.push(`${rustFile} must exist`);
  } else {
    add(violations, trigger(rust, "push") === undefined, `${rustFile} draft checks must not run on push`);
    add(violations, includesAll(at(rust, "on", "pull_request", "paths"), ["Cargo.lock", "Cargo.toml", "crates/**"]), `${rustFile} must cover workspace source changes`);
    const job = requireJob(violations, rustFile, rust, "linux-draft");
    add(violations, job["runs-on"] === "ubuntu-latest", `${rustFile} must use one Ubuntu lane`);
    requireStepRun(violations, rustFile, job, "Check formatting", ["cargo fmt --check"]);
    requireStepRun(violations, rustFile, job, "Check the workspace", ["cargo check --workspace --locked"]);
    requireStepRun(violations, rustFile, job, "Lint workspace libraries", ["cargo clippy --workspace --lib --locked -- -D warnings"]);
    requireStepRun(violations, rustFile, job, "Prove focused publication contracts", ["cargo test --locked"]);
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

function validateReleaseCoordinator(workflows, violations) {
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
  requireStepRun(violations, releaseFile, policy, "Enforce workflow policy", ["node .github/scripts/check-workflow-policy.mjs"]);

  const preflight = requireJob(violations, releaseFile, release, "preflight");
  add(violations, sameMembers(needs(preflight), ["workflow-policy"]), `${releaseFile} preflight must need workflow-policy`);
  requireStepRun(violations, releaseFile, preflight, "Validate versioned changelog notes", [
    "node .github/scripts/extract-codestory-release-notes.mjs",
    '--version "$VERSION"',
  ]);

  const evidence = requireJob(violations, releaseFile, release, "release-evidence");
  add(violations, sameMembers(needs(evidence), ["preflight"]), `${releaseFile} release-evidence must need preflight`);
  for (const [key, value] of Object.entries({
    ref: "${{ github.sha }}",
    proof_key: "release-${{ needs.preflight.outputs.version }}",
    profile: "codestory-release-evidence-linux-arm64-v1",
    drill_manifest: "/srv/codestory-release-evidence/drills/real-repo-drill-cases.json",
    embedding_model_dir: "/srv/codestory-release-evidence/models",
  })) {
    add(violations, object(evidence.with)[key] === value, `${releaseFile} release-evidence with.${key} must equal ${value}`);
  }

  const packaged = requireJob(violations, releaseFile, release, "packaged-proof");
  add(violations, packaged.uses === "./.github/workflows/packaged-platform-proof.yml", `${releaseFile} packaged-proof must call the package workflow`);
  add(violations, sameMembers(needs(packaged), ["preflight", "release-evidence"]), `${releaseFile} packaged-proof must wait for preflight and release evidence`);
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
  add(violations, sameMembers(needs(metal), ["preflight", "packaged-proof"]), `${releaseFile} Metal proof must wait for packaged proof`);
  add(violations, object(metal.with).use_packaged_cli_artifact === true, `${releaseFile} Metal proof must use the packaged CLI`);

  const publish = requireJob(violations, releaseFile, release, "publish");
  add(violations, sameMembers(needs(publish), ["preflight", "packaged-proof", "macos-metal-proof"]), `${releaseFile} publish must wait for all release proofs`);
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
  add(violations, sameMembers(needs(post), ["preflight", "publish"]), `${releaseFile} post-publish smoke must wait for publish`);
}

function expectedPackageRows() {
  return [
    { os: "ubuntu-latest", rust_target: "x86_64-unknown-linux-gnu", asset_target: "linux-x64", exe_suffix: "", extension: "tar.gz" },
    { os: "ubuntu-24.04-arm", rust_target: "aarch64-unknown-linux-gnu", asset_target: "linux-arm64", exe_suffix: "", extension: "tar.gz" },
    { os: "windows-latest", rust_target: "x86_64-pc-windows-msvc", asset_target: "windows-x64", exe_suffix: ".exe", extension: "zip" },
    { os: "windows-11-arm", rust_target: "aarch64-pc-windows-msvc", asset_target: "windows-arm64", exe_suffix: ".exe", extension: "zip" },
    { os: "macos-15-intel", rust_target: "x86_64-apple-darwin", asset_target: "macos-x64", exe_suffix: "", extension: "tar.gz" },
    { os: "macos-15", rust_target: "aarch64-apple-darwin", asset_target: "macos-arm64", exe_suffix: "", extension: "tar.gz" },
  ];
}

function validatePackageMatrixExpression(violations, expression) {
  const match = typeof expression === "string" && expression.match(
    /fromJSON\(inputs\.scope == 'windows' && '([^']+)' \|\| inputs\.scope == 'macos' && '([^']+)' \|\| '([^']+)'\)/u,
  );
  if (!match) {
    violations.push("packaged-platform-proof.yml matrix must select structural JSON by scope");
    return;
  }
  const full = expectedPackageRows();
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

function validatePackagedProof(workflows, violations) {
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
  const job = requireJob(violations, file, workflow, "build");
  validatePackageMatrixExpression(violations, at(job, "strategy", "matrix"));
  add(violations, String(job.environment ?? "").includes("macos-release-signing"), `${file} signed Mac cells must use the protected signing environment`);
  requireStepRun(violations, file, job, "Build Linux x64 at the glibc 2.31 baseline", [
    "cargo build --release --locked -p codestory-cli",
    "CARGO_TARGET_DIR=/workspace/target/glibc-2.31",
  ]);
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
  requireStepRun(violations, file, job, "Run Windows installer ownership self-test", ["scripts/install-codestory.ps1 -SelfTest"]);
  requireStepRun(violations, file, job, "Prove Linux x64 glibc 2.31 baseline", ["bash .github/scripts/check-linux-glibc-baseline.sh"]);
  violations.push(...managedPluginViolations(
    job,
    '--archive "target/release-dist/codestory-cli-v${{ inputs.version }}-${{ matrix.asset_target }}.${{ matrix.extension }}"',
  ).map(message => `${file} ${message}`));
  requireStepUses(violations, file, job, "Upload release asset", "actions/upload-artifact@v7.0.1");
  requireStepUses(violations, file, job, "Upload macOS notarization proof", "actions/upload-artifact@v7.0.1");
}

function validatePostPublish(workflows, violations) {
  const file = "post-publish-release-smoke.yml";
  const workflow = workflows.get(file);
  if (!workflow) {
    violations.push(`${file} must exist`);
    return;
  }
  add(violations, trigger(workflow, "workflow_call") !== undefined, `${file} must be reusable`);
  const job = requireJob(violations, file, workflow, "smoke");
  const expected = expectedPackageRows().map(({ os, asset_target, extension }) => ({ os, asset_target, extension }));
  add(violations, JSON.stringify(at(job, "strategy", "matrix", "include")) === JSON.stringify(expected), `${file} must smoke all six native assets`);
  violations.push(...managedPluginViolations(
    job,
    '--archive "${{ steps.asset.outputs.archive }}"',
  ).map(message => `${file} ${message}`));
  requireStepRun(violations, file, job, "Prove published macOS signature, notarization, and Gatekeeper acceptance", [
    "archive-quarantine.txt",
    "extracted-binary-quarantine.txt",
    "Authority=Developer ID Application:",
    "source=Notarized Developer ID",
    "TeamIdentifier=${APPLE_DEVELOPER_TEAM_ID}",
    "certificate leaf",
  ]);
  requireStepRun(violations, file, job, "Run Windows installer ownership self-test", ["scripts/install-codestory.ps1 -SelfTest"]);
  requireStepRun(violations, file, job, "Prove published Intel macOS backend policy and explicit CPU/external operation", ["--intel-runtime-policy"]);
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
    add(violations, includesAll(at(auto, "on", "push", "paths"), ["package.json", "package-lock.json"]), `${autoFile} must observe policy dependency changes`);
    const policy = requireJob(violations, autoFile, auto, "workflow-policy");
    requireStepRun(violations, autoFile, policy, "Install workflow policy dependencies", ["npm ci --ignore-scripts"]);
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
    violations.push(...releaseEvidenceApprovalViolations(
      [
        ["release.yml", at(workflows.get("release.yml"), "jobs", "release-evidence"), true],
        ["packaged-platform-pr.yml", at(workflows.get("packaged-platform-pr.yml"), "jobs", "release-evidence"), false],
      ],
      evidence,
    ));
    requireStepRun(violations, evidenceFile, job, "Produce full-sidecar repo evidence", ["--test-threads=1"]);
    requireStepRun(violations, evidenceFile, job, "Download prior rejected evidence for approval re-evaluation", ["actions/runs/$SOURCE_RUN_ID", "actions/runs/$SOURCE_RUN_ID/artifacts"]);
  }

  const metalFile = "macos-metal-proof.yml";
  const metal = workflows.get(metalFile);
  if (!metal) {
    violations.push(`${metalFile} must exist`);
  } else {
    add(violations, trigger(metal, "workflow_call") !== undefined && trigger(metal, "workflow_dispatch") !== undefined, `${metalFile} must support reusable and manual proof`);
    const job = requireJob(violations, metalFile, metal, "packaged-metal-lifecycle");
    add(violations, JSON.stringify(job["runs-on"]) === JSON.stringify(["self-hosted", "macOS", "ARM64", "codestory-metal"]), `${metalFile} must use the protected Apple Silicon runner`);
    add(violations, job.environment === "macos-metal-release", `${metalFile} must use the protected Metal environment`);
    requireStepRun(violations, metalFile, job, "Capture host evidence", ["python3 --version", 'test "$macos_major" -ge 15']);
    const lifecycle = namedStep(job, "Prove cold, warm, dead-endpoint, recovery, packet, and plugin lifecycle");
    requireStepRun(violations, metalFile, job, "Prove cold, warm, dead-endpoint, recovery, packet, and plugin lifecycle", ["--native-accelerator-lifecycle", "--managed-plugin-grounding-convergence"]);
    add(violations, object(lifecycle?.env).CODESTORY_EMBED_DEVICE_PROVIDER === "metal", `${metalFile} lifecycle must require Metal`);
    add(violations, object(lifecycle?.env).CODESTORY_EMBED_ALLOW_CPU === "0", `${metalFile} lifecycle must reject CPU fallback`);
    requireStepRun(violations, metalFile, job, "Clean and assert proof-owned hardware state", ["--cleanup-proof-temp-root"]);
  }

  const statsFile = "repo-scale-stats.yml";
  const stats = workflows.get(statsFile);
  if (!stats) {
    violations.push(`${statsFile} must exist`);
  } else {
    const job = requireJob(violations, statsFile, stats, "stats");
    requireStepRun(violations, statsFile, job, "Build the release CLI", ["cargo build --release --locked -p codestory-cli"]);
    requireStepRun(violations, statsFile, job, "Run mandatory repo-scale stats once", ["cargo test --locked -p codestory-cli --test codestory_repo_e2e_stats -- --ignored --nocapture"]);
    requireStepUses(violations, statsFile, job, "Upload repo-scale stats output", "actions/upload-artifact@v7.0.1");
  }

  const retrievalFile = "retrieval-sidecar-smoke.yml";
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

export function validateWorkflows(workflows) {
  const violations = [];
  for (const [file, workflow] of workflows) {
    violations.push(...basicWorkflowViolations(file, workflow));
  }
  validateLockedSetupSurfaces(violations);
  validateIssueWorkflows(workflows, violations);
  validatePluginAndDraftWorkflows(workflows, violations);
  validateReleaseCoordinator(workflows, violations);
  validatePackagedProof(workflows, violations);
  validatePostPublish(workflows, violations);
  validatePackagedCoordinator(workflows, violations);
  validateRemainingWorkflows(workflows, violations);
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
