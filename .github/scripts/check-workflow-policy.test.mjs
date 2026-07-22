import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { chmodSync, mkdtempSync, readFileSync, writeFileSync } from "node:fs";
import os from "node:os";
import path from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";
import {
  basicWorkflowViolations,
  draftSourcePolicyViolations,
  draftWorkflowPolicyViolations,
  loadWorkflows,
  macosCliDistributionViolations,
  managedPluginViolations,
  notaryStepViolations,
  packagedPrSigningViolations,
  parseWorkflow,
  releaseEvidenceApprovalViolations,
  releaseEvidenceWorkflowRef,
  releaseWorkflowContractViolations,
  retrievalFile,
  retrievalProducerTriggerPolicyViolations,
  validateWorkflows,
  windowsManifestProofPolicyViolations,
} from "./check-workflow-policy.mjs";

const fullSha = "0123456789abcdef0123456789abcdef01234567";
const proofTopology = "proof5-v1-64015a841a2f69f33f7c9ce284f671ad27b3923a58db865fd4806d86230df6c5";
const cacheManifestIdentity = "${{ hashFiles('Cargo.toml', 'crates/**/Cargo.toml', 'vendor/**/Cargo.toml') }}";
const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "../..");

function draftSourceJob() {
  return structuredClone(loadWorkflows().get("rust-ci.yml").jobs["linux-draft"]);
}

function draftSourceWorkflow() {
  return structuredClone(loadWorkflows().get("rust-ci.yml"));
}

function retrievalSourceJob() {
  return structuredClone(loadWorkflows().get(retrievalFile).jobs["linux-contracts"]);
}

function retrievalSourceWorkflow() {
  return structuredClone(loadWorkflows().get(retrievalFile));
}

function windowsManifestWorkflow() {
  return retrievalSourceWorkflow();
}

function draftStep(job, name) {
  const matches = job.steps.filter(step => step.name === name);
  assert.equal(matches.length, 1, `expected one ${name} step`);
  return matches[0];
}

function moveNamedStepBefore(job, movedName, beforeName) {
  const movedIndex = job.steps.findIndex(step => step.name === movedName);
  assert.notEqual(movedIndex, -1, `missing ${movedName}`);
  const [moved] = job.steps.splice(movedIndex, 1);
  const beforeIndex = job.steps.findIndex(step => step.name === beforeName);
  assert.notEqual(beforeIndex, -1, `missing ${beforeName}`);
  job.steps.splice(beforeIndex, 0, moved);
}

function cloneCacheSaveBefore(job, sourceName, beforeName, uses) {
  const clone = structuredClone(draftStep(job, sourceName));
  clone.name = `${sourceName} clone`;
  if (uses !== undefined) clone.uses = uses;
  const beforeIndex = job.steps.findIndex(step => step.name === beforeName);
  assert.notEqual(beforeIndex, -1, `missing ${beforeName}`);
  job.steps.splice(beforeIndex, 0, clone);
}

function runResolver(file, jobName, environment) {
  const workflow = loadWorkflows().get(file);
  const run = draftStep(workflow.jobs[jobName], "Resolve trusted exact head").run;
  const directory = mkdtempSync(path.join(os.tmpdir(), "codestory-proof-resolver-"));
  const fakeGh = path.join(directory, "gh");
  const baseSha = "1".repeat(40);
  writeFileSync(fakeGh, `#!/bin/sh
case "$*" in
  *"branches/dev/codestory-next"*) printf '%s\\n' '${fullSha}' ;;
  *) printf '%s\\n' '${JSON.stringify({
    head: {
      repo: { full_name: "TheGreenCedar/CodeStory" },
      sha: fullSha,
      ref: "codex/exact-head",
    },
    base: { sha: baseSha, ref: "dev/codestory-next" },
    labels: [{ name: "review-accepted" }],
  })}' ;;
esac
`);
  chmodSync(fakeGh, 0o755);
  const output = path.join(directory, "github-output");
  writeFileSync(output, "");
  return spawnSync("bash", ["-c", run], {
    encoding: "utf8",
    env: {
      ...process.env,
      PATH: `${directory}:${process.env.PATH}`,
      GH_TOKEN: "test-token",
      GITHUB_REPOSITORY: "TheGreenCedar/CodeStory",
      GITHUB_OUTPUT: output,
      ...environment,
    },
  });
}

function runReleaseAuthority(environment, liveSha = fullSha) {
  const workflow = loadWorkflows().get("release.yml");
  const run = draftStep(workflow.jobs.preflight, "Validate release authority").run;
  const quote = value => `'${String(value).replaceAll("'", `'"'"'`)}'`;
  const exports = Object.entries({
    GH_TOKEN: "test-token",
    GITHUB_REPOSITORY: "TheGreenCedar/CodeStory",
    ...environment,
  })
    .map(([key, value]) => `export ${key}=${quote(value)}`)
    .join("\n");
  const command = `gh() { printf '%s\\n' ${quote(liveSha)}; }
${exports}
${run}`;
  const executable = process.platform === "win32" ? "wsl.exe" : "bash";
  const args = process.platform === "win32"
    ? ["--exec", "/bin/bash", "-c", command]
    : ["-c", command];
  return spawnSync(executable, args, {
    encoding: "utf8",
  });
}

function windowsManifestJob(workflow) {
  return workflow.jobs["windows-manifest-missing"];
}

function windowsManifestStep(workflow, name) {
  return draftStep(windowsManifestJob(workflow), name);
}

function managedJob() {
  return {
    strategy: { "fail-fast": false, matrix: { include: [{ os: "ubuntu-latest" }] } },
    steps: [
      {
        name: "Prove managed plugin handoff",
        env: { CODESTORY_EMBED_ALLOW_CPU: "1" },
        run: [
          "python .github/scripts/check-packaged-agent-proof.py",
          "--archive package.tar.gz",
          "--plugin-handoff",
          "--engine-policy cpu_explicit",
          "--expected-backend CPU",
          "--offline",
          "--timeout-secs 1800",
        ].join("\n"),
      },
    ],
  };
}

function releaseEvidenceApprovalBoundary() {
  return {
    callers: [
      ["release.yml", {
        uses: releaseEvidenceWorkflowRef,
        with: { source_run_id: "${{ inputs.source_run_id }}" },
        secrets: {
          CODESTORY_RELEASE_EVIDENCE_APPROVAL_JSON:
            "${{ secrets.CODESTORY_RELEASE_EVIDENCE_APPROVAL_JSON }}",
        },
      }, true],
      ["packaged-platform-pr.yml", {
        uses: releaseEvidenceWorkflowRef,
        with: { source_run_id: "${{ inputs.source_run_id }}" },
      }, false],
    ],
    called: {
      on: {
        workflow_call: {
          secrets: {
            CODESTORY_RELEASE_EVIDENCE_APPROVAL_JSON: { required: false },
          },
        },
      },
      jobs: {
        measure: {
          environment: "release-evidence",
          steps: [
            {
              name: "Produce and evaluate same-SHA candidate",
              env: {
                APPROVAL_JSON: "${{ secrets.CODESTORY_RELEASE_EVIDENCE_APPROVAL_JSON }}",
              },
              run: [
                'if [ -n "$SOURCE_RUN_ID" ] && [ -z "$APPROVAL_JSON" ]; then',
                '  echo "::error::Protected release-evidence approval is required for source-run re-evaluation."',
                "  exit 1",
                "fi",
              ].join("\n"),
            },
          ],
        },
      },
    },
  };
}

test("parser ignores YAML comments and harmless formatting", () => {
  const block = parseWorkflow(`
on:
  pull_request:
permissions:
  contents: read
jobs:
  check:
    runs-on: ubuntu-latest
    steps:
      - uses: vendor/action@${fullSha}
# uses: vendor/action@main
`);
  const flow = parseWorkflow(`
"on": { pull_request: null }
permissions: { contents: read }
jobs: { check: { runs-on: ubuntu-latest, steps: [ { uses: vendor/action@${fullSha} } ] } }
`);
  assert.deepEqual(block, flow);
});

test("release workflows retain the closeout coordinator contract test", () => {
  assert.deepEqual(validateWorkflows(loadWorkflows()), []);
  for (const [file, jobName] of [
    ["plugin-static.yml", "plugin-static"],
    ["release.yml", "workflow-policy"],
    ["auto-release.yml", "workflow-policy"],
  ]) {
    const workflows = loadWorkflows();
    const step = workflows.get(file).jobs[jobName].steps.find(
      ({ name }) => name === "Check release claim and evidence contracts",
    );
    step.run = step.run.replace("scripts/tests/codestory-release-closeout.test.mjs", "");
    assert.ok(
      validateWorkflows(workflows).some((message) =>
        message.includes(file)
          && message.includes("scripts/tests/codestory-release-closeout.test.mjs")),
    );
  }
});

test("third-party action policy reads only parsed uses values", () => {
  const valid = parseWorkflow(`
on: { workflow_dispatch: null }
jobs:
  check:
    steps:
      - uses: vendor/action@${fullSha}
# uses: vendor/action@main
`);
  assert.deepEqual(basicWorkflowViolations("fixture.yml", valid), []);

  const invalid = structuredClone(valid);
  invalid.jobs.check.steps[0].uses = "vendor/action@main";
  assert.match(basicWorkflowViolations("fixture.yml", invalid).join("\n"), /full-length SHA/u);
});

test("release authority accepts only exact live auto-main or manual-dev routes", async (t) => {
  const auto = {
    EXPECTED_HEAD_SHA: "",
    GITHUB_EVENT_NAME: "push",
    GITHUB_REF: "refs/heads/main",
    GITHUB_SHA: fullSha,
    GITHUB_WORKFLOW_REF: "TheGreenCedar/CodeStory/.github/workflows/auto-release.yml@refs/heads/main",
    PUBLISH_RELEASE: "true",
  };
  const manual = {
    EXPECTED_HEAD_SHA: fullSha,
    GITHUB_EVENT_NAME: "workflow_dispatch",
    GITHUB_REF: "refs/heads/dev/codestory-next",
    GITHUB_SHA: fullSha,
    GITHUB_WORKFLOW_REF: "TheGreenCedar/CodeStory/.github/workflows/release.yml@refs/heads/dev/codestory-next",
    PUBLISH_RELEASE: "",
  };

  await t.test("trusted auto push on live main", () => {
    const result = runReleaseAuthority(auto);
    assert.equal(result.status, 0, result.stderr || result.stdout);
  });
  await t.test("manual proof on exact live dev", () => {
    const result = runReleaseAuthority(manual);
    assert.equal(result.status, 0, result.stderr || result.stdout);
  });
  await t.test("manual event cannot claim publication", () => {
    const result = runReleaseAuthority({ ...manual, PUBLISH_RELEASE: "true" });
    assert.notEqual(result.status, 0);
    assert.match(result.stdout, /Publication authority requires the trusted reusable-workflow caller/u);
  });
  await t.test("wrong automatic caller is rejected", () => {
    const result = runReleaseAuthority({
      ...auto,
      GITHUB_WORKFLOW_REF: "TheGreenCedar/CodeStory/.github/workflows/rogue.yml@refs/heads/main",
    });
    assert.notEqual(result.status, 0);
  });
  await t.test("stale main is rejected", () => {
    const result = runReleaseAuthority(auto, "2".repeat(40));
    assert.notEqual(result.status, 0);
    assert.match(result.stdout, /main moved from release head/u);
  });
  await t.test("wrong manual SHA is rejected", () => {
    const result = runReleaseAuthority({ ...manual, EXPECTED_HEAD_SHA: "2".repeat(40) });
    assert.notEqual(result.status, 0);
    assert.match(result.stdout, /does not match workflow head/u);
  });
  await t.test("stale dev is rejected", () => {
    const result = runReleaseAuthority(manual, "2".repeat(40));
    assert.notEqual(result.status, 0);
    assert.match(result.stdout, /dev\/codestory-next moved from proved head/u);
  });
});

test("proof resolvers reject hostile refs, SHAs, and labeled-event drift before proof work", async (t) => {
  const otherSha = "2".repeat(40);
  const sourceEnvironment = {
    PR_NUMBER: "1230",
    EXPECTED_HEAD_SHA: fullSha,
    CALLER_REF: "",
    EVENT_PR_NUMBER: "",
    EVENT_HEAD_SHA: "",
    EVENT_HEAD_REPO: "",
    GITHUB_EVENT_NAME: "workflow_dispatch",
    GITHUB_SHA: fullSha,
  };
  await t.test("source PR dispatch", () => {
    const rejected = runResolver("source-proof.yml", "resolve", {
      ...sourceEnvironment,
      GITHUB_REF: "refs/heads/main",
    });
    assert.notEqual(rejected.status, 0);
    assert.match(rejected.stdout, /--ref codex\/exact-head/u);

    const wrongSha = runResolver("source-proof.yml", "resolve", {
      ...sourceEnvironment,
      GITHUB_REF: "refs/heads/codex/exact-head",
      GITHUB_SHA: otherSha,
    });
    assert.notEqual(wrongSha.status, 0);
    assert.match(wrongSha.stdout, /Workflow SHA .* is not reviewed PR head/u);

    const accepted = runResolver("source-proof.yml", "resolve", {
      ...sourceEnvironment,
      GITHUB_REF: "refs/heads/codex/exact-head",
    });
    assert.equal(accepted.status, 0, accepted.stderr || accepted.stdout);
  });

  await t.test("source labeled event", () => {
    const environment = {
      PR_NUMBER: "",
      EXPECTED_HEAD_SHA: "",
      CALLER_REF: "",
      EVENT_PR_NUMBER: "1230",
      EVENT_HEAD_SHA: fullSha,
      EVENT_HEAD_REPO: "TheGreenCedar/CodeStory",
      GITHUB_EVENT_NAME: "pull_request",
      GITHUB_REF: "refs/pull/1230/merge",
      GITHUB_SHA: fullSha,
    };
    const accepted = runResolver("source-proof.yml", "resolve", environment);
    assert.equal(accepted.status, 0, accepted.stderr || accepted.stdout);

    const drifted = runResolver("source-proof.yml", "resolve", {
      ...environment,
      EVENT_HEAD_SHA: otherSha,
    });
    assert.notEqual(drifted.status, 0);
    assert.match(drifted.stdout, /moved after the review-accepted label event/u);
  });

  const packagedEnvironment = {
    INPUT_PR_NUMBER: "1230",
    INPUT_HEAD_SHA: fullSha,
    INPUT_MODE: "platform",
    EVENT_PR_NUMBER: "",
    EVENT_HEAD_SHA: "",
    EVENT_HEAD_REPO: "",
    INPUT_SOURCE_RUN_ID: "",
    INPUT_CALIBRATION_ARTIFACT: "",
    INPUT_CALIBRATION_RUN_ID: "",
    GITHUB_EVENT_NAME: "workflow_dispatch",
    GITHUB_SHA: fullSha,
  };
  await t.test("platform PR dispatch", () => {
    const rejected = runResolver("packaged-platform-pr.yml", "route", {
      ...packagedEnvironment,
      GITHUB_REF: "refs/heads/main",
    });
    assert.notEqual(rejected.status, 0);
    assert.match(rejected.stdout, /--ref codex\/exact-head/u);

    const wrongSha = runResolver("packaged-platform-pr.yml", "route", {
      ...packagedEnvironment,
      GITHUB_REF: "refs/heads/codex/exact-head",
      GITHUB_SHA: otherSha,
    });
    assert.notEqual(wrongSha.status, 0);
    assert.match(wrongSha.stdout, /Workflow SHA .* is not accepted PR head/u);

    const accepted = runResolver("packaged-platform-pr.yml", "route", {
      ...packagedEnvironment,
      GITHUB_REF: "refs/heads/codex/exact-head",
    });
    assert.equal(accepted.status, 0, accepted.stderr || accepted.stdout);
  });

  await t.test("platform labeled event", () => {
    const environment = {
      ...packagedEnvironment,
      INPUT_PR_NUMBER: "",
      INPUT_HEAD_SHA: "",
      INPUT_MODE: "",
      EVENT_PR_NUMBER: "1230",
      EVENT_HEAD_SHA: fullSha,
      EVENT_HEAD_REPO: "TheGreenCedar/CodeStory",
      GITHUB_EVENT_NAME: "pull_request",
      GITHUB_REF: "refs/pull/1230/merge",
    };
    const accepted = runResolver("packaged-platform-pr.yml", "route", environment);
    assert.equal(accepted.status, 0, accepted.stderr || accepted.stdout);

    const drifted = runResolver("packaged-platform-pr.yml", "route", {
      ...environment,
      EVENT_HEAD_SHA: otherSha,
    });
    assert.notEqual(drifted.status, 0);
    assert.match(drifted.stdout, /moved after the platform-proof label event/u);
  });

  await t.test("integration dispatch", () => {
    const rejected = runResolver("packaged-platform-pr.yml", "route", {
      ...packagedEnvironment,
      INPUT_PR_NUMBER: "",
      INPUT_MODE: "integration",
      GITHUB_REF: "refs/heads/main",
    });
    assert.notEqual(rejected.status, 0);
    assert.match(rejected.stdout, /--ref dev\/codestory-next/u);

    const accepted = runResolver("packaged-platform-pr.yml", "route", {
      ...packagedEnvironment,
      INPUT_PR_NUMBER: "",
      INPUT_MODE: "integration",
      GITHUB_REF: "refs/heads/dev/codestory-next",
    });
    assert.equal(accepted.status, 0, accepted.stderr || accepted.stdout);
  });
});

test("exact proof policy rejects trigger, identity, and cache downgrades", async (t) => {
  const sourceFile = "source-proof.yml";
  const packagedCoordinatorFile = "packaged-platform-pr.yml";
  const packagedProofFile = "packaged-platform-proof.yml";
  const sourceResolver = workflow => draftStep(workflow.jobs.resolve, "Resolve trusted exact head");
  const packagedResolver = workflow => draftStep(workflow.jobs.route, "Resolve trusted exact head");

  const mutations = [
    ["source synchronize trigger", sourceFile, workflow => {
      workflow.on.pull_request.types.push("synchronize");
    }, /trigger must be label-only/u],
    ["platform synchronize trigger", packagedCoordinatorFile, workflow => {
      workflow.on.pull_request.types.push("synchronize");
    }, /trigger must be label-only/u],
    ["source PR-number-only concurrency", sourceFile, workflow => {
      workflow.concurrency.group = "source-proof-${{ inputs.pr_number || github.event.pull_request.number }}";
    }, /concurrency must bind the Actions SHA/u],
    ["platform PR-number-only concurrency", packagedCoordinatorFile, workflow => {
      workflow.concurrency.group = "proof-${{ inputs.mode }}-${{ inputs.pr_number }}";
    }, /concurrency must bind the Actions SHA/u],
    ["source manual SHA equality", sourceFile, workflow => {
      sourceResolver(workflow).run = sourceResolver(workflow).run
        .replace('test "$GITHUB_SHA" = "$EXPECTED_HEAD_SHA"', 'test -n "$GITHUB_SHA"');
    }, /GITHUB_SHA.*EXPECTED_HEAD_SHA/u],
    ["source manual SHA short-circuit", sourceFile, workflow => {
      sourceResolver(workflow).run = sourceResolver(workflow).run
        .replace(
          'test "$GITHUB_SHA" = "$EXPECTED_HEAD_SHA" || {',
          'true || test "$GITHUB_SHA" = "$EXPECTED_HEAD_SHA" || {',
        );
    }, /exact normalized trusted resolver script contract/u],
    ["source labeled branch disabled", sourceFile, workflow => {
      sourceResolver(workflow).run = sourceResolver(workflow).run
        .replace(
          'if [ -n "$EVENT_PR_NUMBER" ]; then',
          'if false && [ -n "$EVENT_PR_NUMBER" ]; then',
        );
    }, /exact normalized trusted resolver script contract/u],
    ["source resolver exits before trusted checks", sourceFile, workflow => {
      sourceResolver(workflow).run = sourceResolver(workflow).run
        .replace(
          "set -euo pipefail",
          'set -euo pipefail\necho "ref=$GITHUB_SHA" >> "$GITHUB_OUTPUT"\nexit 0',
        );
    }, /exact normalized trusted resolver script contract/u],
    ["source resolver blank line", sourceFile, workflow => {
      sourceResolver(workflow).run = sourceResolver(workflow).run
        .replace("set -euo pipefail\n", "set -euo pipefail\n\n");
    }, /exact normalized trusted resolver script contract/u],
    ["source labeled job disabled", sourceFile, workflow => {
      workflow.jobs.resolve.if
        = "false && (github.event.action == 'labeled' && github.event.label.name == 'review-accepted')";
    }, /only review-accepted labeled PR runs/u],
    ["source manual ref equality", sourceFile, workflow => {
      sourceResolver(workflow).run = sourceResolver(workflow).run
        .replace('test "$GITHUB_REF" = "refs\/heads\/$head_ref"', 'test -n "$GITHUB_REF"');
    }, /GITHUB_REF.*head_ref/u],
    ["platform manual SHA equality", packagedCoordinatorFile, workflow => {
      packagedResolver(workflow).run = packagedResolver(workflow).run
        .replace('test "$GITHUB_SHA" = "$INPUT_HEAD_SHA"', 'test -n "$GITHUB_SHA"');
    }, /GITHUB_SHA.*INPUT_HEAD_SHA/u],
    ["platform manual SHA short-circuit", packagedCoordinatorFile, workflow => {
      packagedResolver(workflow).run = packagedResolver(workflow).run
        .replace(
          'test "$GITHUB_SHA" = "$INPUT_HEAD_SHA" || {',
          'true || test "$GITHUB_SHA" = "$INPUT_HEAD_SHA" || {',
        );
    }, /exact normalized trusted resolver script contract/u],
    ["platform labeled branch disabled", packagedCoordinatorFile, workflow => {
      packagedResolver(workflow).run = packagedResolver(workflow).run
        .replace(
          'if [ -n "$EVENT_HEAD_REPO" ]; then',
          'if false && [ -n "$EVENT_HEAD_REPO" ]; then',
        );
    }, /exact normalized trusted resolver script contract/u],
    ["platform resolver exits before trusted checks", packagedCoordinatorFile, workflow => {
      packagedResolver(workflow).run = packagedResolver(workflow).run
        .replace(
          "set -euo pipefail",
          'set -euo pipefail\necho "head_sha=$GITHUB_SHA" >> "$GITHUB_OUTPUT"\nexit 0',
        );
    }, /exact normalized trusted resolver script contract/u],
    ["platform resolver backslash continuation blank line", packagedCoordinatorFile, workflow => {
      packagedResolver(workflow).run = packagedResolver(workflow).run
        .replace(
          'if [ -n "$INPUT_SOURCE_RUN_ID" ] \\\n    ||',
          'if [ -n "$INPUT_SOURCE_RUN_ID" ] \\\n\n    ||',
        );
    }, /exact normalized trusted resolver script contract/u],
    ["platform labeled job disabled", packagedCoordinatorFile, workflow => {
      workflow.jobs.route.if
        = "false && (github.event.action == 'labeled' && github.event.label.name == 'platform-proof')";
    }, /only platform-proof labeled PR runs/u],
    ["integration live dev SHA equality", packagedCoordinatorFile, workflow => {
      packagedResolver(workflow).run = packagedResolver(workflow).run
        .replace('test "$GITHUB_SHA" = "$dev_head"', 'test -n "$GITHUB_SHA"');
    }, /GITHUB_SHA.*dev_head/u],
    ["hosted-only integration scope removed", packagedCoordinatorFile, workflow => {
      workflow.on.workflow_dispatch.inputs.scope.options
        = workflow.on.workflow_dispatch.inputs.scope.options.filter(scope => scope !== "none");
    }, /dispatch scopes changed/u],
    ["exact integration Linux scope removed", packagedCoordinatorFile, workflow => {
      const step = draftStep(workflow.jobs.route, "Select change-aware proof scope");
      step.run = step.run.replace(' || [ "$REQUESTED_SCOPE" = linux ]', "");
    }, /integration must preserve explicit hosted and Linux scopes/u],
    ["hosted-only release evidence guard removed", packagedCoordinatorFile, workflow => {
      workflow.jobs["release-evidence"].if
        = workflow.jobs["release-evidence"].if.replace("needs.route.outputs.scope != 'none' &&", "");
    }, /hosted-only integration must skip release evidence/u],
    ["Linux release evidence guard removed", packagedCoordinatorFile, workflow => {
      workflow.jobs["release-evidence"].if
        = workflow.jobs["release-evidence"].if.replace("needs.route.outputs.scope != 'linux' &&", "");
    }, /Linux server-behavior proof must not require protected release evidence/u],
    ["Windows release evidence guard removed", packagedCoordinatorFile, workflow => {
      workflow.jobs["release-evidence"].if
        = workflow.jobs["release-evidence"].if.replace("needs.route.outputs.scope != 'windows' &&", "");
    }, /Windows server-behavior proof must not require protected release evidence/u],
    ["Windows package remains blocked on release evidence", packagedCoordinatorFile, workflow => {
      workflow.jobs["packaged-proof"].if
        = workflow.jobs["packaged-proof"].if.replace("needs.route.outputs.scope == 'windows'", "needs.route.outputs.scope == 'full'");
    }, /Windows server-behavior package proof must accept skipped protected release evidence/u],
    ["Windows closeout still requires release evidence", packagedCoordinatorFile, workflow => {
      const step = draftStep(workflow.jobs.closeout, "Require one coherent accepted proof");
      step.run = step.run.replace(' && [ "$SCOPE" != windows ]', "");
    }, /Require one coherent accepted proof/u],
    ["Linux package matrix scope removed", packagedProofFile, workflow => {
      workflow.jobs.build.strategy.matrix
        = workflow.jobs.build.strategy.matrix.replace("inputs.scope == 'linux'", "inputs.scope == 'windows'");
    }, /matrix must select structural JSON by scope/u],
    ["Linux candidate install guard removed", packagedProofFile, workflow => {
      const step = draftStep(workflow.jobs.build, "Stage isolated candidate-managed Linux install");
      step.if = step.if.replace("inputs.scope == 'linux' || ", "");
    }, /remain runnable in server and Linux scopes/u],
    ["source unversioned cache", sourceFile, workflow => {
      const restore = draftStep(workflow.jobs["full-source-gate"], "Restore Cargo inputs and output");
      restore.with.key = restore.with.key.replace("source-proof-v2", "source-proof");
    }, /versioned exact-SHA namespace/u],
    ["source broad cache fallback", sourceFile, workflow => {
      draftStep(workflow.jobs["full-source-gate"], "Restore Cargo inputs and output")
        .with["restore-keys"] = "Linux-source-proof-";
    }, /must not use fallback restore keys/u],
    ["source cache always-save", sourceFile, workflow => {
      draftStep(workflow.jobs["full-source-gate"], "Save Cargo inputs and output").if
        = "always() && steps.cargo-cache-restore.outputs.cache-hit != 'true'";
    }, /save only a successful exact miss/u],
    ["source cache save before final proof", sourceFile, workflow => {
      moveNamedStepBefore(
        workflow.jobs["full-source-gate"],
        "Save Cargo inputs and output",
        "Test the complete workspace once",
      );
    }, /source cache unique cache save must run after every proof step/u],
    ["source cache clone before proof", sourceFile, workflow => {
      cloneCacheSaveBefore(
        workflow.jobs["full-source-gate"],
        "Save Cargo inputs and output",
        "Test the complete workspace once",
      );
    }, /source cache must contain exactly one actions\/cache\/save action/u],
    ["source mixed-case cache clone before proof", sourceFile, workflow => {
      cloneCacheSaveBefore(
        workflow.jobs["full-source-gate"],
        "Save Cargo inputs and output",
        "Test the complete workspace once",
        "Actions/Cache/Save@v5",
      );
    }, /source cache must contain exactly one actions\/cache\/save action/u],
    ["macOS source cache loses exact SHA", packagedCoordinatorFile, workflow => {
      const restore = draftStep(workflow.jobs["macos-source"], "Restore exact-head macOS source cache");
      restore.with.key = restore.with.key.replace("-${{ needs.route.outputs.head_sha }}", "");
    }, /macOS source cache must be an exact-SHA restore/u],
    ["macOS source cache save before final proof", packagedCoordinatorFile, workflow => {
      moveNamedStepBefore(
        workflow.jobs["macos-source"],
        "Save exact-head macOS source cache",
        "Capture Rust cache identity",
      );
    }, /macOS source cache unique cache save must run after every proof step/u],
    ["macOS source cache clone before proof", packagedCoordinatorFile, workflow => {
      cloneCacheSaveBefore(
        workflow.jobs["macos-source"],
        "Save exact-head macOS source cache",
        "Capture Rust cache identity",
      );
    }, /macOS source cache must contain exactly one actions\/cache\/save action/u],
    ["macOS source mixed-case cache clone before proof", packagedCoordinatorFile, workflow => {
      cloneCacheSaveBefore(
        workflow.jobs["macos-source"],
        "Save exact-head macOS source cache",
        "Capture Rust cache identity",
        "Actions/Cache/Save@v5",
      );
    }, /macOS source cache must contain exactly one actions\/cache\/save action/u],
    ["packaged cache loses exact SHA", packagedProofFile, workflow => {
      const restore = draftStep(workflow.jobs.build, "Restore Cargo registry, git sources, and build output");
      restore.with.key = restore.with.key.replace("-${{ inputs.ref }}", "");
    }, /native build cache.*exact SHA/u],
    ["packaged cache always-save", packagedProofFile, workflow => {
      draftStep(workflow.jobs.build, "Save Cargo registry, git sources, and build output").if
        = "always() && steps.cargo-cache-restore.outputs.cache-hit != 'true'";
    }, /save only a successful exact miss/u],
    ["packaged cache save before final proof", packagedProofFile, workflow => {
      moveNamedStepBefore(
        workflow.jobs.build,
        "Save Cargo registry, git sources, and build output",
        "Build codestory-cli",
      );
    }, /native build cache unique cache save must run after every proof and cleanup step/u],
    ["packaged cache clone before proof", packagedProofFile, workflow => {
      cloneCacheSaveBefore(
        workflow.jobs.build,
        "Save Cargo registry, git sources, and build output",
        "Build codestory-cli",
      );
    }, /native build cache must contain exactly one actions\/cache\/save action/u],
    ["packaged mixed-case cache clone before proof", packagedProofFile, workflow => {
      cloneCacheSaveBefore(
        workflow.jobs.build,
        "Save Cargo registry, git sources, and build output",
        "Build codestory-cli",
        "Actions/Cache/Save@v5",
      );
    }, /native build cache must contain exactly one actions\/cache\/save action/u],
  ];

  assert.deepEqual(validateWorkflows(loadWorkflows()), []);
  for (const [name, file, mutate, expectedReason] of mutations) {
    await t.test(name, () => {
      const workflows = loadWorkflows();
      mutate(workflows.get(file));
      assert.match(validateWorkflows(workflows).join("\n"), expectedReason);
    });
  }
});

test("hosted Linux calibration keeps its bounded per-run timeout", async (t) => {
  assert.deepEqual(validateWorkflows(loadWorkflows()), []);

  for (const [name, replacement] of [
    ["removed", ""],
    ["shortened", "--timeout-secs 900"],
  ]) {
    await t.test(name, () => {
      const workflows = loadWorkflows();
      const packaged = workflows.get("packaged-platform-proof.yml");
      const step = draftStep(
        packaged.jobs.build,
        "Packaged per-user server calibration or qualification",
      );
      step.run = step.run.replace("--timeout-secs 1800", replacement);

      assert.match(
        validateWorkflows(workflows).join("\n"),
        /step Packaged per-user server calibration or qualification must run --timeout-secs 1800/u,
      );
    });
  }
});

test("Cargo lock policy reads executable step commands", () => {
  const workflow = parseWorkflow(`
on: { workflow_dispatch: null }
jobs:
  check:
    steps:
      - run: |
          # cargo test --workspace
          cargo test --workspace --locked
`);
  assert.deepEqual(basicWorkflowViolations("fixture.yml", workflow), []);

  workflow.jobs.check.steps[0].run += "\ncargo check --workspace\n";
  assert.match(basicWorkflowViolations("fixture.yml", workflow).join("\n"), /must use --locked/u);
});

test("draft source cache reuse preserves exact serial proof structure", async (t) => {
  assert.deepEqual(draftSourcePolicyViolations(draftSourceJob(), retrievalSourceJob()), []);

  const mutations = [
    ["unversioned primary", job => {
      const step = draftStep(job, "Restore Cargo inputs and output");
      step.with.key = step.with.key.replace("-draft-v2-", "-draft-");
    }],
    ["lock-only primary", job => {
      const step = draftStep(job, "Restore Cargo inputs and output");
      step.with.key = step.with.key.replace(`${cacheManifestIdentity}-`, "");
    }],
    ["mismatched proof topology", job => {
      const step = draftStep(job, "Restore Cargo inputs and output");
      step.with.key = step.with.key.replace(proofTopology, proofTopology.replace("-v1-", "-v2-"));
    }],
    ["fallback order reversal", job => {
      const step = draftStep(job, "Restore Cargo inputs and output");
      step.with["restore-keys"] = step.with["restore-keys"].trim().split("\n").reverse().join("\n");
    }],
    ["overbroad draft fallback", job => {
      const step = draftStep(job, "Restore Cargo inputs and output");
      const keys = step.with["restore-keys"].trim().split("\n");
      keys[1] = "${{ runner.os }}-draft-v2-";
      step.with["restore-keys"] = keys.join("\n");
    }],
    ["cross-platform fallback", job => {
      const step = draftStep(job, "Restore Cargo inputs and output");
      step.with["restore-keys"] = step.with["restore-keys"].replace("${{ runner.os }}-cargo-stable-", "Windows-cargo-stable-");
    }],
    ["all-feature fallback", job => {
      const step = draftStep(job, "Restore Cargo inputs and output");
      step.with["restore-keys"] = step.with["restore-keys"].replace("-default-features-", "-all-features-");
    }],
    ["source-proof fallback", job => {
      const step = draftStep(job, "Restore Cargo inputs and output");
      step.with["restore-keys"] = step.with["restore-keys"].replace("-retrieval-contracts-", "-source-proof-");
    }],
    ["manifest-free prior retrieval fallback", job => {
      const step = draftStep(job, "Restore Cargo inputs and output");
      const keys = step.with["restore-keys"].trim().split("\n");
      keys[2] = keys[2].replace(`${cacheManifestIdentity}-`, "");
      step.with["restore-keys"] = keys.join("\n");
    }],
    ["target-free prior draft fallback", job => {
      const step = draftStep(job, "Restore Cargo inputs and output");
      const keys = step.with["restore-keys"].trim().split("\n");
      keys[1] = keys[1].replace("-${{ steps.rust-cache-key.outputs.target }}-", "-");
      step.with["restore-keys"] = keys.join("\n");
    }],
    ["different restore path", job => {
      const step = draftStep(job, "Restore Cargo inputs and output");
      step.with.path = step.with.path.replace("target", "target/release");
    }],
    ["blocking restore", job => {
      draftStep(job, "Restore Cargo inputs and output")["continue-on-error"] = false;
    }],
    ["matched-key save", job => {
      draftStep(job, "Save Cargo inputs and output").with.key = "${{ steps.cargo-cache-restore.outputs.cache-matched-key }}";
    }],
    ["promotion before complete proof", job => {
      draftStep(job, "Save Cargo inputs and output").if = "steps.cargo-cache-restore.outputs.cache-hit != 'true'";
    }],
    ["removed proof command", job => {
      const step = draftStep(job, "Prove focused publication contracts");
      step.run = step.run.trim().split("\n").slice(0, -1).join("\n");
    }],
    ["reordered proof commands", job => {
      const step = draftStep(job, "Prove focused publication contracts");
      const commands = step.run.trim().split("\n");
      [commands[0], commands[1]] = [commands[1], commands[0]];
      step.run = commands.join("\n");
    }],
    ["backgrounded Cargo command", job => {
      const step = draftStep(job, "Check the workspace");
      step.run = `${step.run} &`;
    }],
    ["parallel Cargo commands", job => {
      const step = draftStep(job, "Check the workspace");
      step.run = `${step.run} &\nwait`;
    }],
    ["reordered proof steps", job => {
      const left = job.steps.findIndex(step => step.name === "Check the workspace");
      const right = job.steps.findIndex(step => step.name === "Lint workspace libraries");
      [job.steps[left], job.steps[right]] = [job.steps[right], job.steps[left]];
    }],
    ["optional proof step", job => {
      draftStep(job, "Lint workspace libraries")["continue-on-error"] = true;
    }],
    ["decoy cache step", job => {
      const restore = draftStep(job, "Restore Cargo inputs and output");
      const decoy = structuredClone(restore);
      decoy.name = "Decoy cache contract";
      restore.with.key = "decoy-primary";
      job.steps.push(decoy);
    }],
  ];

  for (const [name, mutate] of mutations) {
    await t.test(name, () => {
      const candidate = draftSourceJob();
      mutate(candidate);
      assert.notDeepEqual(draftSourcePolicyViolations(candidate, retrievalSourceJob()), []);
    });
  }

  for (const [name, mutate] of [
    ["incompatible retrieval path", job => {
      draftStep(job, "Restore Cargo registry, git sources, and build output").with.path = "~/.cargo/registry\ntarget/retrieval\n";
    }],
    ["incompatible retrieval key", job => {
      const step = draftStep(job, "Restore Cargo registry, git sources, and build output");
      step.with.key = step.with.key.replace("-default-features-", "-all-features-");
    }],
    ["mismatched retrieval topology version", job => {
      const step = draftStep(job, "Restore Cargo registry, git sources, and build output");
      step.with.key = step.with.key.replace(proofTopology, proofTopology.replace("-v1-", "-v2-"));
    }],
    ["manifest-free retrieval key", job => {
      const step = draftStep(job, "Restore Cargo registry, git sources, and build output");
      step.with.key = step.with.key.replace(`${cacheManifestIdentity}-`, "");
    }],
    ["incompatible retrieval action", job => {
      draftStep(job, "Restore Cargo registry, git sources, and build output").uses = "actions/cache/restore@v4";
    }],
    ["omitted seed target", job => {
      const step = draftStep(job, "Seed draft proof test-profile artifacts");
      step.run = step.run.trim().split("\n").slice(1).join("\n");
    }],
    ["reordered seed targets", job => {
      const step = draftStep(job, "Seed draft proof test-profile artifacts");
      const commands = step.run.trim().split("\n");
      [commands[0], commands[1]] = [commands[1], commands[0]];
      step.run = commands.join("\n");
    }],
    ["executable seed target", job => {
      const step = draftStep(job, "Seed draft proof test-profile artifacts");
      step.run = step.run.replace(" --no-run", "");
    }],
    ["optional seed step", job => {
      draftStep(job, "Seed draft proof test-profile artifacts")["continue-on-error"] = true;
    }],
    ["save before seed", job => {
      const seed = job.steps.findIndex(step => step.name === "Seed draft proof test-profile artifacts");
      const save = job.steps.findIndex(step => step.name === "Save Cargo registry, git sources, and build output");
      [job.steps[seed], job.steps[save]] = [job.steps[save], job.steps[seed]];
    }],
    ["producer matched-key save", job => {
      draftStep(job, "Save Cargo registry, git sources, and build output").with.key = "${{ steps.cargo-cache-restore.outputs.cache-matched-key }}";
    }],
  ]) {
    await t.test(name, () => {
      const candidate = retrievalSourceJob();
      mutate(candidate);
      assert.notDeepEqual(draftSourcePolicyViolations(draftSourceJob(), candidate), []);
    });
  }
});

test("retrieval cache producer triggers cover every draft manifest consumer", async (t) => {
  assert.deepEqual(retrievalProducerTriggerPolicyViolations(retrievalSourceWorkflow()), []);

  const reordered = retrievalSourceWorkflow();
  reordered.on.pull_request.paths.reverse();
  reordered.on.push.paths.reverse();
  assert.deepEqual(
    retrievalProducerTriggerPolicyViolations(reordered),
    [],
    "required trigger membership is order-insensitive",
  );

  const requiredPaths = [
    "crates/**/Cargo.toml",
    "vendor/**/Cargo.toml",
    ".github/workflows/rust-ci.yml",
  ];
  for (const event of ["pull_request", "push"]) {
    for (const requiredPath of requiredPaths) {
      await t.test(`${event} rejects removal of ${requiredPath}`, () => {
        const candidate = retrievalSourceWorkflow();
        candidate.on[event].paths = candidate.on[event].paths
          .filter(triggerPath => triggerPath !== requiredPath);
        assert.notDeepEqual(retrievalProducerTriggerPolicyViolations(candidate), []);
        const workflows = loadWorkflows();
        workflows.set(retrievalFile, candidate);
        assert.match(
          validateWorkflows(workflows).join("\n"),
          /retrieval cache producer .* paths must cover/u,
        );
      });
    }
  }

  await t.test("push must retain the dev branch", () => {
    const candidate = retrievalSourceWorkflow();
    candidate.on.push.branches = candidate.on.push.branches
      .filter(branch => branch !== "dev/codestory-next");
    assert.notDeepEqual(retrievalProducerTriggerPolicyViolations(candidate), []);
    const workflows = loadWorkflows();
    workflows.set(retrievalFile, candidate);
    assert.match(
      validateWorkflows(workflows).join("\n"),
      /retrieval cache producer must run on dev\/codestory-next pushes/u,
    );
  });
});

test("Windows manifest-missing proof freezes routing, native topology, and exact cache identity", async (t) => {
  assert.deepEqual(windowsManifestProofPolicyViolations(windowsManifestWorkflow()), []);

  const keyStep = workflow => windowsManifestStep(
    workflow,
    "Restore Windows Cargo inputs and output",
  );
  const proofStep = workflow => windowsManifestStep(
    workflow,
    "Prove Windows ready_command manifest-missing contract",
  );
  const saveStep = workflow => windowsManifestStep(
    workflow,
    "Save Windows Cargo inputs and output",
  );
  const installerHash = "${{ hashFiles('.github/scripts/install-windows-vulkan-sdk.ps1') }}";
  const lockHash = "${{ hashFiles('Cargo.lock') }}";

  const mutations = [
    ["cloned Windows job routed on pull requests", workflow => {
      const clone = structuredClone(windowsManifestJob(workflow));
      clone.if = "github.event_name == 'pull_request'";
      clone["continue-on-error"] = true;
      workflow.jobs["windows-manifest-decoy"] = clone;
    }, /must contain exactly linux-contracts and windows-manifest-missing jobs/u],
    ["top-level build target", workflow => {
      workflow.env = { CARGO_BUILD_TARGET: "x86_64-pc-windows-gnu" };
    }, /must not define top-level env/u],
    ["top-level shell default", workflow => {
      workflow.defaults = { run: { shell: "bash" } };
    }, /must not define top-level defaults/u],
    ["top-level working-directory default", workflow => {
      workflow.defaults = { run: { "working-directory": "crates/codestory-cli" } };
    }, /must not define top-level defaults/u],
    ["pull request omits installer", workflow => {
      workflow.on.pull_request.paths = workflow.on.pull_request.paths
        .filter(triggerPath => triggerPath !== ".github/scripts/install-windows-vulkan-sdk.ps1");
    }],
    ["push omits installer", workflow => {
      workflow.on.push.paths = workflow.on.push.paths
        .filter(triggerPath => triggerPath !== ".github/scripts/install-windows-vulkan-sdk.ps1");
    }],
    ["dispatch inputs", workflow => {
      workflow.on.workflow_dispatch = { inputs: { ref: { required: false, type: "string" } } };
    }],
    ["pull-request job routing", workflow => {
      windowsManifestJob(workflow).if = "github.event_name == 'pull_request'";
    }],
    ["label routing", workflow => {
      windowsManifestJob(workflow).if = "contains(github.event.pull_request.labels.*.name, 'proof')";
    }],
    ["older runner", workflow => {
      windowsManifestJob(workflow)["runs-on"] = "windows-2022";
    }],
    ["longer timeout", workflow => {
      windowsManifestJob(workflow)["timeout-minutes"] = 60;
    }],
    ["CPU permission removed", workflow => {
      delete windowsManifestJob(workflow).env.CODESTORY_EMBED_ALLOW_CPU;
    }],
    ["CPU permission disabled", workflow => {
      windowsManifestJob(workflow).env.CODESTORY_EMBED_ALLOW_CPU = "0";
    }],
    ["native generator removed", workflow => {
      delete windowsManifestJob(workflow).env.CMAKE_GENERATOR;
    }],
    ["native generator changed to Visual Studio", workflow => {
      windowsManifestJob(workflow).env.CMAKE_GENERATOR = "Visual Studio 18 2026";
    }],
    ["native generator moved to proof-step override", workflow => {
      delete windowsManifestJob(workflow).env.CMAKE_GENERATOR;
      proofStep(workflow).env = { CMAKE_GENERATOR: "Ninja" };
    }],
    ["extra product feature environment", workflow => {
      windowsManifestJob(workflow).env.CARGO_FEATURES = "cpu-only";
    }],
    ["job made optional", workflow => {
      windowsManifestJob(workflow)["continue-on-error"] = true;
    }],
    ["checkout alternate ref", workflow => {
      windowsManifestJob(workflow).steps[0].with = { ref: "main" };
    }],
    ["installer removed", workflow => {
      windowsManifestJob(workflow).steps = windowsManifestJob(workflow).steps
        .filter(step => step.name !== "Install checksum-pinned Windows Vulkan SDK");
    }],
    ["installer replaced", workflow => {
      windowsManifestStep(workflow, "Install checksum-pinned Windows Vulkan SDK").run = "choco install vulkan-sdk";
    }],
    ["installer made optional", workflow => {
      windowsManifestStep(workflow, "Install checksum-pinned Windows Vulkan SDK")["continue-on-error"] = true;
    }],
    ["installer moved after proof", workflow => {
      const job = windowsManifestJob(workflow);
      const installer = job.steps.findIndex(step => step.name === "Install checksum-pinned Windows Vulkan SDK");
      const proof = job.steps.findIndex(step => step.name === "Prove Windows ready_command manifest-missing contract");
      [job.steps[installer], job.steps[proof]] = [job.steps[proof], job.steps[installer]];
    }],
    ["CMake cache identity capture removed", workflow => {
      const identity = windowsManifestStep(workflow, "Capture Rust cache identity");
      identity.run = identity.run.replace(/.*cmake.*\n/gu, "");
    }],
    ["Ninja cache identity capture removed", workflow => {
      const identity = windowsManifestStep(workflow, "Capture Rust cache identity");
      identity.run = identity.run.replace(/.*ninja.*\n/gu, "");
    }],
    ["unversioned proof topology", workflow => {
      keyStep(workflow).with.key = keyStep(workflow).with.key
        .replace(/ready-command-v2-[0-9a-f]{64}/u, "ready-command");
    }],
    ["stale proof topology", workflow => {
      keyStep(workflow).with.key = keyStep(workflow).with.key
        .replace("ready-command-v2-", "ready-command-v1-");
    }],
    ["generator-free cache", workflow => {
      keyStep(workflow).with.key = keyStep(workflow).with.key
        .replace("-generator-ninja", "");
    }],
    ["CMake-free cache", workflow => {
      keyStep(workflow).with.key = keyStep(workflow).with.key
        .replace("-cmake-${{ steps.rust-cache-key.outputs.cmake }}", "");
    }],
    ["Ninja-free cache", workflow => {
      keyStep(workflow).with.key = keyStep(workflow).with.key
        .replace("-ninja-${{ steps.rust-cache-key.outputs.ninja }}", "");
    }],
    ["OS-free cache", workflow => {
      keyStep(workflow).with.key = keyStep(workflow).with.key.replace("${{ runner.os }}-", "");
    }],
    ["Rust-free cache", workflow => {
      keyStep(workflow).with.key = keyStep(workflow).with.key
        .replace("-${{ steps.rust-cache-key.outputs.version }}", "");
    }],
    ["target-free cache", workflow => {
      keyStep(workflow).with.key = keyStep(workflow).with.key
        .replace("-${{ steps.rust-cache-key.outputs.target }}", "");
    }],
    ["all-feature cache", workflow => {
      keyStep(workflow).with.key = keyStep(workflow).with.key
        .replace("-default-features-", "-all-features-");
    }],
    ["manifest-free cache", workflow => {
      keyStep(workflow).with.key = keyStep(workflow).with.key
        .replace(`${cacheManifestIdentity}-`, "");
    }],
    ["installer-free cache", workflow => {
      keyStep(workflow).with.key = keyStep(workflow).with.key.replace(`${installerHash}-`, "");
    }],
    ["lock-free cache", workflow => {
      keyStep(workflow).with.key = keyStep(workflow).with.key.replace(lockHash, "unlocked");
    }],
    ["fallback cache prefix", workflow => {
      keyStep(workflow).with["restore-keys"] = "Windows-cargo-stable-";
    }],
    ["alternate cache output", workflow => {
      keyStep(workflow).with.path = "target/windows";
    }],
    ["cache restore bypass", workflow => {
      keyStep(workflow).if = "always()";
    }],
    ["unlocked proof", workflow => {
      proofStep(workflow).run = proofStep(workflow).run.replace(" --locked", "");
    }],
    ["supplied-binary substitute", workflow => {
      proofStep(workflow).run = "cargo test --locked -p codestory-cli --test ready_command --features supplied-binary";
    }],
    ["proof made optional", workflow => {
      proofStep(workflow)["continue-on-error"] = true;
    }],
    ["save before proof", workflow => {
      const job = windowsManifestJob(workflow);
      const proof = job.steps.findIndex(step => step.name === "Prove Windows ready_command manifest-missing contract");
      const save = job.steps.findIndex(step => step.name === "Save Windows Cargo inputs and output");
      [job.steps[proof], job.steps[save]] = [job.steps[save], job.steps[proof]];
    }],
    ["save after failed proof", workflow => {
      saveStep(workflow).if = "steps.cargo-cache-restore.outputs.cache-hit != 'true'";
    }],
    ["save exact hit", workflow => {
      saveStep(workflow).if = "success()";
    }],
    ["save matched key", workflow => {
      saveStep(workflow).with.key = "${{ steps.cargo-cache-restore.outputs.cache-matched-key }}";
    }],
    ["save fallback input", workflow => {
      saveStep(workflow).with["restore-keys"] = "Windows-cargo-stable-";
    }],
    ["decoy proof", workflow => {
      const decoy = structuredClone(proofStep(workflow));
      proofStep(workflow).run = "Write-Output skipped";
      decoy.name = "Decoy ready_command proof";
      windowsManifestJob(workflow).steps.push(decoy);
    }],
  ];

  for (const [name, mutate, expectedReason = /Windows manifest proof/u] of mutations) {
    await t.test(name, () => {
      const candidate = windowsManifestWorkflow();
      mutate(candidate);
      const violations = windowsManifestProofPolicyViolations(candidate);
      assert.notDeepEqual(violations, []);
      assert.match(violations.join("\n"), expectedReason);
      const workflows = loadWorkflows();
      workflows.set(retrievalFile, candidate);
      assert.match(
        validateWorkflows(workflows).join("\n"),
        expectedReason,
      );
    });
  }
});

test("Windows source package builds pin Ninja and bind native tool identity", async (t) => {
  assert.deepEqual(validateWorkflows(loadWorkflows()), []);

  const packagedFile = "packaged-platform-proof.yml";
  const protectedFile = "windows-vulkan-proof.yml";
  const packagedIdentity = workflow => draftStep(workflow.jobs.build, "Capture Rust cache key");
  const packagedCache = workflow => draftStep(
    workflow.jobs.build,
    "Restore Cargo registry, git sources, and build output",
  );
  const packagedBuild = workflow => draftStep(workflow.jobs.build, "Build codestory-cli");
  const packagedShortTarget = workflow => draftStep(
    workflow.jobs.build,
    "Configure short Windows Cargo target",
  );
  const protectedSourceTools = workflow => draftStep(
    workflow.jobs["packaged-vulkan"],
    "Capture source build tool evidence",
  );
  const protectedBuild = workflow => draftStep(
    workflow.jobs["packaged-vulkan"],
    "Build and package native CLI",
  );

  const mutations = [
    ["packaged CMake identity removed", packagedFile, workflow => {
      packagedIdentity(workflow).run = packagedIdentity(workflow).run
        .replace(/.*cmake --version.*\n/u, "");
    }, /native build identity must include cmake/u],
    ["packaged Ninja identity removed", packagedFile, workflow => {
      packagedIdentity(workflow).run = packagedIdentity(workflow).run
        .replace(/.*ninja --version.*\n/u, "");
    }, /native build identity must include ninja/u],
    ["packaged Ninja selection removed", packagedFile, workflow => {
      packagedIdentity(workflow).run = packagedIdentity(workflow).run
        .replace(/.*CMAKE_GENERATOR=Ninja.*\n/u, "");
    }, /native build identity must include CMAKE_GENERATOR=Ninja/u],
    ["packaged short Windows target made cross-platform", packagedFile, workflow => {
      packagedShortTarget(workflow).if = "runner.os != 'Windows'";
    }, /short Cargo target must be Windows-only/u],
    ["packaged short Windows target stops using a junction", packagedFile, workflow => {
      packagedShortTarget(workflow).run = packagedShortTarget(workflow).run
        .replace("New-Item -ItemType Junction", "New-Item -ItemType Directory");
    }, /Configure short Windows Cargo target/u],
    ["packaged short Windows target stops using the runner volume root", packagedFile, workflow => {
      packagedShortTarget(workflow).run = packagedShortTarget(workflow).run
        .replace("$runnerRoot = [System.IO.Path]::GetPathRoot($workspaceTarget)", "$runnerRoot = $env:RUNNER_TEMP");
    }, /Configure short Windows Cargo target/u],
    ["packaged short Windows target points at wrong storage", packagedFile, workflow => {
      packagedShortTarget(workflow).run = packagedShortTarget(workflow).run
        .replace("-Target $workspaceTarget", '-Target "wrong"');
    }, /Configure short Windows Cargo target/u],
    ["packaged short Windows target no longer exports Cargo output", packagedFile, workflow => {
      packagedShortTarget(workflow).run = packagedShortTarget(workflow).run
        .replace("| Out-File -FilePath $env:GITHUB_ENV", "| Write-Output");
    }, /Configure short Windows Cargo target/u],
    ["packaged identity made conditional", packagedFile, workflow => {
      packagedIdentity(workflow).if = "runner.os != 'Windows'";
    }, /native build identity must be unique, unconditional/u],
    ["packaged identity made optional", packagedFile, workflow => {
      packagedIdentity(workflow)["continue-on-error"] = true;
    }, /native build identity must be unique, unconditional/u],
    ["packaged identity cloned", packagedFile, workflow => {
      workflow.jobs.build.steps.push(structuredClone(packagedIdentity(workflow)));
    }, /native build identity must be unique, unconditional/u],
    ["packaged identity moved after build", packagedFile, workflow => {
      const steps = workflow.jobs.build.steps;
      const identityIndex = steps.findIndex(step => step.name === "Capture Rust cache key");
      const [identity] = steps.splice(identityIndex, 1);
      const buildIndex = steps.findIndex(step => step.name === "Build codestory-cli");
      steps.splice(buildIndex + 1, 0, identity);
    }, /native build identity must run immediately after Rust selection/u],
    ["packaged generator-free cache", packagedFile, workflow => {
      packagedCache(workflow).with.key = packagedCache(workflow).with.key
        .replace("-${{ steps.rust-cache-key.outputs.generator }}", "");
    }, /native build cache must bind generator, CMake, Ninja/u],
    ["packaged CMake-free cache", packagedFile, workflow => {
      packagedCache(workflow).with.key = packagedCache(workflow).with.key
        .replace("-cmake-${{ steps.rust-cache-key.outputs.cmake }}", "");
    }, /native build cache must bind generator, CMake, Ninja/u],
    ["packaged Ninja-free cache", packagedFile, workflow => {
      packagedCache(workflow).with.key = packagedCache(workflow).with.key
        .replace("-ninja-${{ steps.rust-cache-key.outputs.ninja }}", "");
    }, /native build cache must bind generator, CMake, Ninja/u],
    ["packaged build overrides generator", packagedFile, workflow => {
      packagedBuild(workflow).env = { CMAKE_GENERATOR: "Visual Studio 18 2026" };
    }, /native package build must not override the selected generator/u],
    ["packaged Windows smoke ignores short target", packagedFile, workflow => {
      draftStep(workflow.jobs.build, "Smoke codestory-cli on Windows").run
        = '$bin = "target/codestory-cli.exe"';
    }, /Smoke codestory-cli on Windows/u],
    ["packaged Windows asset ignores short target", packagedFile, workflow => {
      draftStep(workflow.jobs.build, "Package release asset on Windows").run
        = 'python .github/scripts/package-codestory-release.py --binary "target/codestory-cli.exe"';
    }, /Package release asset on Windows/u],
    ["packaged Windows asset reroutes the short-target binary", packagedFile, workflow => {
      const step = draftStep(workflow.jobs.build, "Package release asset on Windows");
      step.run = step.run.replace("--binary $bin", "--binary target/wrong.exe");
    }, /Package release asset on Windows/u],
    ["protected generator removed", protectedFile, workflow => {
      delete protectedBuild(workflow).env.CMAKE_GENERATOR;
    }, /source package build must use the Ninja native generator/u],
    ["protected generator changed", protectedFile, workflow => {
      protectedBuild(workflow).env.CMAKE_GENERATOR = "Visual Studio 18 2026";
    }, /source package build must use the Ninja native generator/u],
    ["protected build adds a second generator surface", protectedFile, workflow => {
      protectedBuild(workflow).env.CMAKE_GENERATOR_PLATFORM = "x64";
    }, /source package build must use the Ninja native generator/u],
    ["protected host omits generator selection", protectedFile, workflow => {
      protectedSourceTools(workflow).run = protectedSourceTools(workflow).run
        .replace(/.*CMAKE_GENERATOR=Ninja.*\n/u, "");
    }, /Capture source build tool evidence/u],
    ["protected host omits CMake version", protectedFile, workflow => {
      protectedSourceTools(workflow).run = protectedSourceTools(workflow).run
        .replace(/.*cmake --version.*\n/u, "");
    }, /Capture source build tool evidence/u],
    ["protected host omits Ninja version", protectedFile, workflow => {
      protectedSourceTools(workflow).run = protectedSourceTools(workflow).run
        .replace(/.*ninja --version.*\n/u, "");
    }, /Capture source build tool evidence/u],
    ["protected source evidence made unconditional", protectedFile, workflow => {
      delete protectedSourceTools(workflow).if;
    }, /source build tool evidence must remain source-only/u],
    ["protected source evidence guard inverted", protectedFile, workflow => {
      protectedSourceTools(workflow).if = "inputs.use_packaged_cli_artifact";
    }, /source build tool evidence must remain source-only/u],
    ["protected source evidence made optional", protectedFile, workflow => {
      protectedSourceTools(workflow)["continue-on-error"] = true;
    }, /source build tool evidence must remain source-only/u],
  ];

  for (const [name, file, mutate, expectedReason] of mutations) {
    await t.test(name, () => {
      const workflows = loadWorkflows();
      mutate(workflows.get(file));
      const violations = validateWorkflows(workflows);
      assert.notDeepEqual(violations, []);
      assert.match(violations.join("\n"), expectedReason);
    });
  }
});

test("Windows candidate-installed proof remains distinct and provenance-bound", async (t) => {
  assert.deepEqual(validateWorkflows(loadWorkflows()), []);

  const coordinatorFile = "packaged-platform-pr.yml";
  const protectedFile = "windows-vulkan-proof.yml";
  const releaseFile = "release.yml";
  const candidateStage = workflow => draftStep(
    workflow.jobs["packaged-vulkan"],
    "Stage isolated candidate-managed Windows install",
  );
  const candidateProof = workflow => draftStep(
    workflow.jobs["packaged-vulkan"],
    "Prove two-host candidate-installed Windows runtime",
  );
  const candidateUpload = workflow => draftStep(
    workflow.jobs["packaged-vulkan"],
    "Upload candidate-installed Windows proof",
  );

  const mutations = [
    ["coordinator opt-in removed", coordinatorFile, workflow => {
      delete workflow.jobs["windows-vulkan-proof"].with.candidate_installed_proof;
    }, /accepted PR Windows package into candidate-installed proof/u],
    ["coordinator server-only scope removed", coordinatorFile, workflow => {
      delete workflow.jobs["windows-vulkan-proof"].with.server_behavior_only;
    }, /non-quality Windows claim only for Windows scope/u],
    ["coordinator quality artifact bypasses producer result", coordinatorFile, workflow => {
      workflow.jobs["windows-vulkan-proof"].with.quality_evidence_artifact
        = "${{ needs.route.outputs.constants_frozen == 'true' && format('release-evidence-{0}', needs.route.outputs.head_sha) || '' }}";
    }, /Windows quality evidence must come only from the successful protected producer/u],
    ["release enables pre-merge proof", releaseFile, workflow => {
      workflow.jobs["windows-vulkan-proof"].with.candidate_installed_proof = true;
    }, /leave pre-merge Windows candidate-installed proof/u],
    ["release enables server-only proof", releaseFile, workflow => {
      workflow.jobs["windows-vulkan-proof"].with.server_behavior_only = true;
    }, /leave pre-merge Windows candidate-installed proof/u],
    ["explicit opt-in removed", protectedFile, workflow => {
      delete workflow.on.workflow_call.inputs.candidate_installed_proof;
    }, /candidate-installed proof must be an explicit opt-in/u],
    ["server-only opt-in removed", protectedFile, workflow => {
      delete workflow.on.workflow_call.inputs.server_behavior_only;
    }, /server-behavior-only claim scope must be an explicit opt-in/u],
    ["candidate staging loses server-only route", protectedFile, workflow => {
      candidateStage(workflow).if = candidateStage(workflow).if
        .replace("inputs.server_behavior_only || ", "");
    }, /candidate-managed staging must require coordinator opt-in and remain runnable in Windows server scope/u],
    ["candidate staging bypassed", protectedFile, workflow => {
      candidateStage(workflow).run = candidateStage(workflow).run
        .replace("--prepare-candidate-installed-proof", "--version-only");
    }, /Stage isolated candidate-managed Windows install/u],
    ["candidate tier weakened", protectedFile, workflow => {
      candidateProof(workflow).run = candidateProof(workflow).run
        .replace("--proof-tier installed_runtime", "--proof-tier protected_hardware");
    }, /Prove two-host candidate-installed Windows runtime/u],
    ["candidate proof loses server-only route", protectedFile, workflow => {
      candidateProof(workflow).if = candidateProof(workflow).if
        .replace("inputs.server_behavior_only || ", "");
    }, /candidate-installed proof must require coordinator opt-in and remain runnable in Windows server scope/u],
    ["candidate server-only claim removed", protectedFile, workflow => {
      candidateProof(workflow).run = candidateProof(workflow).run
        .replace("--server-behavior-only", "--version-only");
    }, /Prove two-host candidate-installed Windows runtime/u],
    ["candidate cell relabeled", protectedFile, workflow => {
      candidateProof(workflow).run = candidateProof(workflow).run
        .replace(
          "candidate_installed_windows_x64_cpu",
          "protected_windows_x64_vulkan",
        );
    }, /Prove two-host candidate-installed Windows runtime/u],
    ["candidate CPU opt-in removed", protectedFile, workflow => {
      candidateProof(workflow).env.CODESTORY_EMBED_ALLOW_CPU = "0";
    }, /explicit CPU execution/u],
    ["candidate provenance removed", protectedFile, workflow => {
      candidateProof(workflow).run = candidateProof(workflow).run
        .replace("--installed-plugin-provenance", "--untrusted-plugin-provenance");
    }, /Prove two-host candidate-installed Windows runtime/u],
    ["candidate artifact loses attempt identity", protectedFile, workflow => {
      candidateUpload(workflow).with.name = "candidate-installed-windows-${{ inputs.version }}";
    }, /attempt-scoped artifact/u],
    ["server-only proof emits release cell", protectedFile, workflow => {
      draftStep(workflow.jobs["packaged-vulkan"], "Emit authenticated Vulkan release cell").if
        = "inputs.emit_release_cells";
    }, /server-behavior-only proof must never emit a release cell/u],
  ];

  for (const [name, file, mutate, expectedReason] of mutations) {
    await t.test(name, () => {
      const workflows = loadWorkflows();
      mutate(workflows.get(file));
      const violations = validateWorkflows(workflows);
      assert.notDeepEqual(violations, []);
      assert.match(violations.join("\n"), expectedReason);
    });
  }
});

test("candidate-installed package qualification retains enough bounded job time", () => {
  const workflows = loadWorkflows();
  assert.deepEqual(validateWorkflows(workflows), []);

  workflows.get("packaged-platform-proof.yml").jobs.build["timeout-minutes"] =
    "${{ inputs.calibration_mode && 180 || (inputs.candidate_installed_proof && (matrix.asset_target == 'linux-x64' || matrix.asset_target == 'windows-x64') && 60 || (inputs.sign_macos && startsWith(matrix.asset_target, 'macos-') && 90 || 60)) }}";

  assert.match(
    validateWorkflows(workflows).join("\n"),
    /x64 candidate-installed package qualification must retain a bounded 120-minute timeout/u,
  );
});

test("draft source workflow freezes its complete top-level contract", async (t) => {
  assert.deepEqual(draftWorkflowPolicyViolations(draftSourceWorkflow()), []);
  const reordered = draftSourceWorkflow();
  [reordered.on.pull_request.paths[0], reordered.on.pull_request.paths[1]]
    = [reordered.on.pull_request.paths[1], reordered.on.pull_request.paths[0]];
  assert.deepEqual(
    draftWorkflowPolicyViolations(reordered),
    [],
    "path membership is exact but order-insensitive",
  );

  const mutations = [
    ["workflow name", workflow => { workflow.name = "Draft checks"; }],
    ["missing pull request trigger", workflow => { delete workflow.on.pull_request; }],
    ["extra push trigger", workflow => { workflow.on.push = { branches: ["main"] }; }],
    ["missing path", workflow => { workflow.on.pull_request.paths.pop(); }],
    ["duplicate path", workflow => {
      workflow.on.pull_request.paths[1] = workflow.on.pull_request.paths[0];
    }],
    ["extra path", workflow => { workflow.on.pull_request.paths.push("scripts/**"); }],
    ["dispatch inputs", workflow => {
      workflow.on.workflow_dispatch = { inputs: { ref: { required: false, type: "string" } } };
    }],
    ["missing dispatch", workflow => { delete workflow.on.workflow_dispatch; }],
    ["write permission", workflow => { workflow.permissions.contents = "write"; }],
    ["extra permission", workflow => { workflow.permissions.actions = "read"; }],
    ["concurrency group", workflow => { workflow.concurrency.group = "draft-${{ github.ref }}"; }],
    ["disabled concurrency cancellation", workflow => {
      workflow.concurrency["cancel-in-progress"] = false;
    }],
    ["extra concurrency field", workflow => { workflow.concurrency.limit = 1; }],
    ["top-level env", workflow => { workflow.env = { CARGO_TERM_COLOR: "always" }; }],
    ["top-level defaults", workflow => {
      workflow.defaults = { run: { shell: "bash" } };
    }],
    ["missing jobs", workflow => { delete workflow.jobs; }],
    ["cloned job", workflow => {
      workflow.jobs["extra-draft-lane"] = structuredClone(workflow.jobs["linux-draft"]);
    }],
  ];

  for (const [name, mutate] of mutations) {
    await t.test(name, () => {
      const candidate = draftSourceWorkflow();
      mutate(candidate);
      assert.notDeepEqual(draftWorkflowPolicyViolations(candidate), []);
    });
  }
});

test("draft source job rejects every alternate execution surface", async (t) => {
  assert.deepEqual(draftSourcePolicyViolations(draftSourceJob(), retrievalSourceJob()), []);

  const mutations = [
    ["job name", job => { job.name = "Draft source"; }],
    ["runner", job => { job["runs-on"] = "ubuntu-24.04"; }],
    ["timeout", job => { job["timeout-minutes"] = 60; }],
    ["if", job => { job.if = "always()"; }],
    ["needs", job => { job.needs = ["untrusted"]; }],
    ["permissions", job => { job.permissions = { contents: "write" }; }],
    ["continue-on-error", job => { job["continue-on-error"] = true; }],
    ["strategy", job => { job.strategy = { matrix: { shard: [1, 2] } }; }],
    ["env", job => { job.env = { RUSTFLAGS: "-Awarnings" }; }],
    ["defaults", job => { job.defaults = { run: { shell: "bash" } }; }],
    ["environment", job => { job.environment = "release"; }],
    ["container", job => { job.container = "ubuntu:latest"; }],
    ["services", job => { job.services = { cache: { image: "redis" } }; }],
    ["outputs", job => { job.outputs = { result: "${{ steps.proof.outputs.result }}" }; }],
  ];

  for (const [name, mutate] of mutations) {
    await t.test(name, () => {
      const candidate = draftSourceJob();
      mutate(candidate);
      assert.notDeepEqual(draftSourcePolicyViolations(candidate, retrievalSourceJob()), []);
    });
  }
});

test("draft source steps reject checkout and proof bypass mutations", async (t) => {
  const checkout = job => job.steps[0];
  const proof = job => draftStep(job, "Prove focused publication contracts");
  const mutations = [
    ["checkout ref", job => { checkout(job).with = { ref: "refs/heads/main" }; }],
    ["checkout persisted credentials", job => {
      checkout(job).with = { "persist-credentials": true };
    }],
    ["checkout if", job => { checkout(job).if = "always()"; }],
    ["checkout continue-on-error", job => { checkout(job)["continue-on-error"] = true; }],
    ["checkout env", job => { checkout(job).env = { GH_TOKEN: "token" }; }],
    ["checkout id", job => { checkout(job).id = "checkout"; }],
    ["checkout action", job => { checkout(job).uses = "actions/checkout@v4"; }],
    ["cloned step", job => { job.steps.push(structuredClone(checkout(job))); }],
    ["deleted step", job => { job.steps.splice(5, 1); }],
    ["reordered steps", job => {
      [job.steps[5], job.steps[6]] = [job.steps[6], job.steps[5]];
    }],
    ["run step shell", job => { draftStep(job, "Check formatting").shell = "bash"; }],
    ["restore extra input", job => {
      draftStep(job, "Restore Cargo inputs and output").with["fail-on-cache-miss"] = false;
    }],
    ["save extra input", job => {
      draftStep(job, "Save Cargo inputs and output").with["restore-keys"] = "decoy";
    }],
    ["proof if", job => { proof(job).if = "always()"; }],
    ["proof continue-on-error", job => { proof(job)["continue-on-error"] = true; }],
    ["proof env", job => { proof(job).env = { RUST_BACKTRACE: "1" }; }],
    ["native staging proof removed", job => {
      proof(job).run = proof(job).run
        .split("\n")
        .filter(command => !command.includes("--test native_staging"))
        .join("\n");
    }],
    ["native staging proof reordered", job => {
      const commands = proof(job).run.trim().split("\n");
      [commands[0], commands[1]] = [commands[1], commands[0]];
      proof(job).run = commands.join("\n");
    }],
  ];

  for (const [name, mutate] of mutations) {
    await t.test(name, () => {
      const candidate = draftSourceJob();
      mutate(candidate);
      assert.notDeepEqual(draftSourcePolicyViolations(candidate, retrievalSourceJob()), []);
    });
  }
});

test("draft source workflow rejects cloned top-level jobs", () => {
  const workflows = loadWorkflows();
  const workflow = draftSourceWorkflow();
  assert.deepEqual(draftWorkflowPolicyViolations(workflow), []);

  workflow.jobs["extra-draft-lane"] = structuredClone(workflow.jobs["linux-draft"]);
  workflows.set("rust-ci.yml", workflow);
  assert.match(
    validateWorkflows(workflows).join("\n"),
    /must contain exactly the linux-draft job/u,
  );
});

test("managed proof rejects structural bypasses and decoy commands", () => {
  assert.deepEqual(managedPluginViolations(managedJob(), "--archive package.tar.gz"), []);

  const mutations = [
    job => { job.strategy["fail-fast"] = true; },
    job => { job.strategy.matrix.exclude = [{ os: "ubuntu-latest" }]; },
    job => { job.if = "always()"; },
    job => { job.steps[0]["continue-on-error"] = true; },
    job => { delete job.steps[0].env.CODESTORY_EMBED_ALLOW_CPU; },
    job => { job.steps[0].run = job.steps[0].run.replace("--engine-policy cpu_explicit", ""); },
    job => { job.steps[0].run = job.steps[0].run.replace("--expected-backend CPU", ""); },
    job => { job.steps[0].run = job.steps[0].run.replace("--offline", ""); },
    job => { job.steps[0].run = job.steps[0].run.replace("--timeout-secs 1800", "--timeout-secs 900"); },
    job => {
      job.steps[0].run = "python .github/scripts/check-packaged-agent-proof.py\n--archive package.tar.gz\n# --plugin-handoff";
      job.steps.push({ name: "Decoy", run: "--plugin-handoff" });
    },
  ];
  for (const mutate of mutations) {
    const candidate = managedJob();
    mutate(candidate);
    assert.notDeepEqual(managedPluginViolations(candidate, "--archive package.tar.gz"), []);
  }
});

test("PR package proof cannot opt into signing credentials", () => {
  const workflow = { jobs: { "packaged-proof": { with: { sign_macos: false } } } };
  assert.deepEqual(packagedPrSigningViolations(workflow), []);

  for (const mutate of [
    candidate => { candidate.jobs["packaged-proof"].with.sign_macos = true; },
    candidate => { candidate.jobs["packaged-proof"].secrets = "inherit"; },
    candidate => { candidate.jobs["packaged-proof"].environment = "macos-release-signing"; },
    candidate => { candidate.env = { APPLE_NOTARY_KEY_ID: "forbidden" }; },
  ]) {
    const candidate = structuredClone(workflow);
    mutate(candidate);
    assert.notDeepEqual(packagedPrSigningViolations(candidate), []);
  }
});

test("release approval crosses only the protected release boundary", () => {
  const boundary = releaseEvidenceApprovalBoundary();
  assert.deepEqual(releaseEvidenceApprovalViolations(boundary.callers, boundary.called), []);

  for (const mutate of [
    candidate => { candidate.callers[0][1] = undefined; },
    candidate => { candidate.callers[1][1].uses = "./.github/workflows/release.yml"; },
    candidate => { delete candidate.callers[1][1].with.source_run_id; },
    candidate => { delete candidate.callers[0][1].secrets; },
    candidate => {
      candidate.callers[0][1].secrets.CODESTORY_RELEASE_EVIDENCE_APPROVAL_JSON
        = "${{ secrets.WRONG_SECRET }}";
    },
    candidate => { candidate.callers[0][1].secrets.EXTRA_SECRET = "${{ secrets.EXTRA }}"; },
    candidate => { candidate.callers[0][1].secrets = "inherit"; },
    candidate => { candidate.callers[1][1].secrets = "inherit"; },
    candidate => { delete candidate.called.on.workflow_call.secrets; },
    candidate => {
      candidate.called.on.workflow_call.secrets
        .CODESTORY_RELEASE_EVIDENCE_APPROVAL_JSON.required = true;
    },
    candidate => { candidate.called.jobs.measure.environment = "release"; },
    candidate => {
      candidate.called.jobs.measure.steps[0].env.APPROVAL_JSON
        = "${{ inputs.CODESTORY_RELEASE_EVIDENCE_APPROVAL_JSON }}";
    },
    candidate => { candidate.called.jobs.measure.steps[0].run = "exit 1"; },
  ]) {
    const candidate = structuredClone(boundary);
    mutate(candidate);
    assert.notDeepEqual(releaseEvidenceApprovalViolations(candidate.callers, candidate.called), []);
  }
});

test("notarization must use explicit polling", () => {
  assert.deepEqual(notaryStepViolations({ run: "xcrun notarytool submit bundle.zip \\\n  --no-wait" }), []);
  assert.match(
    notaryStepViolations({ run: "xcrun notarytool submit bundle.zip \\\n  --wait" }).join("\n"),
    /poll explicitly/u,
  );
});

test("bare macOS CLI proof uses quarantine execution instead of app assessment", () => {
  const assessment = {
    run: [
      "xattr -w com.apple.quarantine quarantine codestory-cli",
      "xattr -p com.apple.quarantine codestory-cli > quarantine.txt",
      "spctl --assess --type execute --verbose=4 codestory-cli > spctl-diagnostic.txt 2>&1",
      "spctl_status=$?",
      "grep -F 'does not seem to be an app' spctl-diagnostic.txt",
    ].join("\n"),
  };
  const execution = { run: "codestory-cli --version\ncodestory-cli --help" };
  assert.deepEqual(macosCliDistributionViolations(assessment, execution, "codestory-cli"), []);

  for (const mutate of [
    candidate => { candidate.assessment.run = candidate.assessment.run.replace("xattr -w com.apple.quarantine quarantine codestory-cli", "true"); },
    candidate => { candidate.assessment.run += "\naccepted=false"; },
    candidate => { candidate.assessment.run = candidate.assessment.run.replace("spctl_status=$?", "true"); },
    candidate => { candidate.execution.run = "original-cli --version\noriginal-cli --help"; },
  ]) {
    const candidate = { assessment: structuredClone(assessment), execution: structuredClone(execution) };
    mutate(candidate);
    assert.notDeepEqual(macosCliDistributionViolations(candidate.assessment, candidate.execution, "codestory-cli"), []);
  }
});

test("controlled semantic workflow fixtures emit class-prefixed diagnostics", async (t) => {
  const fixture = JSON.parse(readFileSync(path.join(
    root,
    ".github/scripts/fixtures/workflow-policy-invalid.json",
  ), "utf8"));
  assert.deepEqual(releaseWorkflowContractViolations(loadWorkflows()), []);
  for (const fixtureCase of fixture.cases) {
    await t.test(fixtureCase.id, () => {
      const workflows = loadWorkflows();
      const workflow = workflows.get(fixtureCase.workflow);
      let target = fixtureCase.job ? workflow.jobs[fixtureCase.job] : workflow;
      if (fixtureCase.step) {
        target = target.steps.find(({ name }) => name === fixtureCase.step);
        assert.ok(target, `missing step ${fixtureCase.step}`);
      }
      const field = [...fixtureCase.field];
      const key = field.pop();
      for (const segment of field) target = target[segment];
      if (fixtureCase.op === "delete") delete target[key];
      else target[key] = structuredClone(fixtureCase.value);
      const violations = releaseWorkflowContractViolations(workflows);
      assert.ok(
        violations.some((message) => message.startsWith(fixtureCase.class_prefix)),
        violations.join("\n"),
      );
    });
  }
});

test("release policy rejects manifest producer, trusted-map, and publication bypasses", () => {
  const mutations = [
    ["call expected head", workflows => { delete workflows.get("release.yml").on.workflow_call.inputs.expected_head_sha; }],
    ["call publication default", workflows => { workflows.get("release.yml").on.workflow_call.inputs.publish_release.default = true; }],
    ["manual expected head", workflows => { workflows.get("release.yml").on.workflow_dispatch.inputs.expected_head_sha.required = false; }],
    ["manual publication authority", workflows => {
      workflows.get("release.yml").on.workflow_dispatch.inputs.publish_release = {
        required: false,
        type: "boolean",
        default: false,
      };
    }],
    ["release authority guard", workflows => {
      const step = workflows.get("release.yml").jobs.preflight.steps
        .find(({ name }) => name === "Validate release authority");
      step.run = step.run.replace("dev/codestory-next moved from proved head", "dev head changed");
    }],
    ["automatic caller event", workflows => {
      const step = workflows.get("release.yml").jobs.preflight.steps
        .find(({ name }) => name === "Validate release authority");
      step.run = step.run.replace('"$GITHUB_EVENT_NAME" != "push"', '"$GITHUB_EVENT_NAME" != "workflow_call"');
    }],
    ["accepted dev ledger revalidation", workflows => {
      workflows.get("release.yml").jobs["pre-publish-closeout"].steps = workflows
        .get("release.yml").jobs["pre-publish-closeout"].steps
        .filter(({ name }) => name !== "Revalidate proof-only dev head");
    }],
    ["publish-time main revalidation", workflows => {
      const step = workflows.get("release.yml").jobs.publish.steps
        .find(({ name }) => name === "Create GitHub release");
      step.run = step.run.replace("main moved from publishable head", "main changed");
    }],
    ["publish authority", workflows => { delete workflows.get("release.yml").jobs.publish.if; }],
    ["post-publish smoke authority", workflows => { delete workflows.get("release.yml").jobs["post-publish-smoke"].if; }],
    ["post-publish closeout authority", workflows => { delete workflows.get("release.yml").jobs["post-publish-closeout"].if; }],
    ["trusted caller opt-in", workflows => { delete workflows.get("auto-release.yml").jobs.release.with.publish_release; }],
    ["rogue release caller", workflows => {
      workflows.get("plugin-static.yml").jobs["rogue-release"] = {
        uses: "./.github/workflows/release.yml",
      };
    }],
    ["source emission", workflows => { delete workflows.get("release.yml").jobs["source-proof"].with.emit_release_cells; }],
    ["full rerun preflight guard", workflows => {
      workflows.get("release.yml").jobs.preflight.steps = workflows
        .get("release.yml").jobs.preflight.steps
        .filter(({ name }) => name !== "Refuse existing tag or release");
    }],
    ["publish replay guard", workflows => {
      const step = workflows.get("release.yml").jobs.publish.steps
        .find(({ name }) => name === "Refuse existing tag or release");
      step.run = step.run.replaceAll("exit 1", "true");
    }],
    ["publish bypass", workflows => {
      workflows.get("release.yml").jobs.publish.needs = [
        "preflight",
        "packaged-proof",
        "macos-metal-proof",
        "windows-vulkan-proof",
      ];
    }],
    ["trusted producer map", workflows => {
      const step = workflows.get("release.yml").jobs["pre-publish-closeout"].steps
        .find(({ name }) => name === "Evaluate authenticated pre-publish closeout");
      step.run = step.run.replace("--trusted-producers", "--self-attested-producers");
    }],
    ["trusted exception input", workflows => {
      const step = workflows.get("release.yml").jobs["pre-publish-closeout"].steps
        .find(({ name }) => name === "Evaluate authenticated pre-publish closeout");
      step.run = step.run.replace("--trusted-exceptions", "--manifest-exceptions");
    }],
    ["flattened current-run JSON", workflows => {
      const step = workflows.get("release.yml").jobs["pre-publish-closeout"].steps
        .find(({ name }) => name === "Download selected pre-publish release cells");
      delete step.with["artifact-ids"];
      step.with.pattern = "release-cell-prepublish-*";
      step.with["merge-multiple"] = true;
    }],
    ["container digest warning accepted", workflows => {
      const step = workflows.get("release.yml").jobs["pre-publish-closeout"].steps
        .find(({ name }) => name === "Verify selected pre-publish artifact container digests");
      step.run = step.run.replace(
        'test "$actual_digest" = "$expected_digest"',
        'echo "$actual_digest $expected_digest"',
      );
    }],
    ["attempt-free artifact", workflows => {
      const step = workflows.get("source-proof.yml").jobs["full-source-gate"].steps
        .find(({ name }) => name === "Upload authenticated source release cell");
      step.with.name = "release-cell-prepublish-source";
    }],
    ["rerun-unsafe diagnostic artifact", workflows => {
      const step = workflows.get("post-publish-release-smoke.yml").jobs.smoke.steps
        .find(({ name }) => name === "Upload post-publish proof artifacts");
      step.with.name = "post-publish-proof-fixed";
    }],
    ["rerun-unsafe stable artifact", workflows => {
      const step = workflows.get("packaged-platform-proof.yml").jobs.build.steps
        .find(({ name }) => name === "Upload release asset");
      delete step.with.overwrite;
    }],
    ["overwriteable terminal evidence", workflows => {
      const step = workflows.get("packaged-platform-proof.yml").jobs.build.steps
        .find(({ name }) => name === "Upload candidate-installed Linux proof");
      step.name = "Upload hosted Linux calibration runs";
      step.with.name = "embedding-calibration-linux-${{ inputs.version }}";
      step.with.path = "target/calibration-runs/linux";
      step.with.overwrite = true;
    }],
    ["attempt-qualified duplicate stable key", workflows => {
      const steps = workflows.get("packaged-platform-proof.yml").jobs.build.steps;
      const index = steps.findIndex(({ name }) => name === "Upload hosted Linux calibration runs");
      steps.splice(index + 1, 0, {
        name: "Upload hosted Linux calibration runs",
        uses: "actions/upload-artifact@v7.0.1",
        with: {
          name: "diagnostic-attempt-${{ github.run_attempt }}",
          path: "forged.json",
          "retention-days": 30,
        },
      });
    }],
    ["rogue artifact producer", workflows => {
      workflows.get("release.yml").jobs["pre-publish-closeout"].steps.push({
        name: "Upload forged release cell",
        uses: "actions/upload-artifact@v7.0.1",
        with: {
          name: "release-cell-prepublish-source-attempt-${{ github.run_attempt }}",
          path: "forged.json",
        },
      });
    }],
    ["pre-publish ledger", workflows => {
      const step = workflows.get("release.yml").jobs["post-publish-closeout"].steps
        .find(({ name }) => name === "Evaluate authenticated post-publish closeout");
      step.run = step.run.replace("--pre-publish-ledger", "--untrusted-ledger");
    }],
    ["success-only post-publish upload", workflows => {
      delete workflows.get("post-publish-release-smoke.yml").jobs.smoke.steps
        .find(({ name }) => name === "Upload authenticated post-publish release cells").if;
    }],
  ];
  for (const [label, mutate] of mutations) {
    const workflows = loadWorkflows();
    mutate(workflows);
    assert.notDeepEqual(validateWorkflows(workflows), [], label);
  }
});
