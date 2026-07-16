import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
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
  validateWorkflows,
} from "./check-workflow-policy.mjs";

const fullSha = "0123456789abcdef0123456789abcdef01234567";
const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "../..");

function draftSourceJob() {
  return structuredClone(loadWorkflows().get("rust-ci.yml").jobs["linux-draft"]);
}

function draftSourceWorkflow() {
  return structuredClone(loadWorkflows().get("rust-ci.yml"));
}

function retrievalSourceJob() {
  return structuredClone(loadWorkflows().get("retrieval-engine-smoke.yml").jobs["linux-contracts"]);
}

function draftStep(job, name) {
  const matches = job.steps.filter(step => step.name === name);
  assert.equal(matches.length, 1, `expected one ${name} step`);
  return matches[0];
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
      step.with.key = step.with.key.replace(
        "hashFiles('Cargo.lock', 'Cargo.toml', 'crates/**/Cargo.toml', 'vendor/**/Cargo.toml')",
        "hashFiles('Cargo.lock')",
      );
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
      step.with["restore-keys"] = step.with["restore-keys"].replace("retrieval-contracts-default-features", "retrieval-contracts-all-features");
    }],
    ["source-proof fallback", job => {
      const step = draftStep(job, "Restore Cargo inputs and output");
      step.with["restore-keys"] = step.with["restore-keys"].replace("retrieval-contracts-default-features", "source-proof-all-targets-all-features");
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
      step.with.key = step.with.key.replace("retrieval-contracts-default-features", "retrieval-contracts-all-features");
    }],
    ["incompatible retrieval action", job => {
      draftStep(job, "Restore Cargo registry, git sources, and build output").uses = "actions/cache/restore@v4";
    }],
  ]) {
    await t.test(name, () => {
      const candidate = retrievalSourceJob();
      mutate(candidate);
      assert.notDeepEqual(draftSourcePolicyViolations(draftSourceJob(), candidate), []);
    });
  }
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
