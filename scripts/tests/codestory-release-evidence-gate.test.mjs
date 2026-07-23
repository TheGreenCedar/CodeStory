import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { copyFileSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import { spawnSync } from "node:child_process";
import test from "node:test";
import { fileURLToPath } from "node:url";
import { validatePacketCorpusContract } from "../codestory-release-evidence-gate.mjs";

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "../..");
const script = path.join(root, "scripts/codestory-release-evidence-gate.mjs");
const fixture = path.join(root, "benchmarks/release-evidence/fixtures");
const baseline = path.join(root, "benchmarks/release-evidence/approved-baselines.json");
const candidateSha = "2222222222222222222222222222222222222222";

function run(command, extra = []) {
  return spawnSync(process.execPath, [script, command, "--baseline", baseline, "--expected-sha", candidateSha, "--release-key", "ci-contract-v1", "--mode", "contract", "--repo", root, ...extra], { encoding: "utf8" });
}
function workspace() {
  const dir = mkdtempSync(path.join(tmpdir(), "codestory-release-evidence-"));
  copyFileSync(path.join(fixture, "candidate-stats.json"), path.join(dir, "stats.json"));
  copyFileSync(path.join(fixture, "candidate-packet.json"), path.join(dir, "packet.json"));
  return dir;
}
function produce(dir) {
  const result = run("produce", ["--profile", "ci-contract-v1", "--stats", path.join(dir, "stats.json"), "--packet", path.join(dir, "packet.json"), "--out", path.join(dir, "candidate.json")]);
  assert.equal(result.status, 0, result.stderr);
}
function reattest(candidate, artifact, filePath) {
  const bytes = readFileSync(filePath);
  candidate.artifacts[artifact].sha256 = createHash("sha256").update(bytes).digest("hex");
  candidate.artifacts[artifact].bytes = bytes.length;
}

test("v0.16 release corpus contract rejects an omitted or substituted packet task", () => {
  const relativePath = "benchmarks/release-evidence/corpus-contracts/v0.16-axios-js-ts-v1.json";
  const bytes = readFileSync(path.join(root, relativePath));
  const contract = JSON.parse(bytes);
  const identity = {
    corpus_id: contract.corpus_id,
    cache_id: "cold-inprocess-v1",
    machine_fingerprint: "fixture/fingerprint",
  };
  const provenance = {
    corpus_contract: {
      path: relativePath,
      sha256: createHash("sha256").update(bytes).digest("hex"),
      corpus_id: contract.corpus_id,
      task_ids: contract.task_ids,
      runtime_modes: contract.runtime_modes,
      repeats: contract.repeats,
      task_manifests: contract.task_manifests,
      task_repositories: { "axios-request-dispatch": "axios" },
      project_manifests: contract.project_manifests,
    },
  };
  const profile = { corpus_contract: { path: relativePath, sha256: provenance.corpus_contract.sha256 } };
  const rows = Array.from({ length: contract.repeats }, (_, index) => ({
    repo: "axios",
    task_id: "axios-request-dispatch",
    mode: "cold_cli_packet",
    repeat: index + 1,
  }));
  assert.doesNotThrow(() => validatePacketCorpusContract(provenance, rows, root, identity, profile));
  assert.throws(
    () => validatePacketCorpusContract(provenance, [], root, identity, profile),
    /do not exactly match the checked-in release task scope/,
  );
  assert.throws(
    () => validatePacketCorpusContract(provenance, rows.map((row) => ({ ...row, repo: "substitute" })), root, identity, profile),
    /do not exactly match the checked-in release task scope/,
  );
  assert.throws(
    () => validatePacketCorpusContract(provenance, [...rows, { ...rows[0], repo: "substitute" }], root, identity, profile),
    /do not exactly match the checked-in release task scope/,
  );
  assert.throws(
    () => validatePacketCorpusContract(provenance, rows.map((row) => ({ ...row, task_id: "ripgrep-search-pipeline" })), root, identity, profile),
    /do not exactly match the checked-in release task scope/,
  );
});

function axiosV2CorpusFixture(mutate) {
  const dir = mkdtempSync(path.join(tmpdir(), "codestory-axios-v2-corpus-"));
  const taskRelative = "benchmarks/tasks/release-evidence/axios-request-dispatch-v2.task.json";
  const projectRelative = "benchmarks/tasks/release-evidence/axios-js-ts-codestory-project-v2.json";
  const contractRelative = "benchmarks/release-evidence/corpus-contracts/v0.16-axios-js-ts-v2.json";
  for (const relative of [taskRelative, projectRelative, contractRelative]) {
    mkdirSync(path.dirname(path.join(dir, relative)), { recursive: true });
    copyFileSync(path.join(root, relative), path.join(dir, relative));
  }
  const paths = {
    task: path.join(dir, taskRelative),
    project: path.join(dir, projectRelative),
    contract: path.join(dir, contractRelative),
  };
  mutate?.(paths);
  return { dir, paths, contractRelative };
}

function rewriteJson(filePath, mutate) {
  const document = JSON.parse(readFileSync(filePath));
  mutate(document);
  writeFileSync(filePath, `${JSON.stringify(document, null, 2)}\n`);
}

function rebindTaskHash(paths) {
  rewriteJson(paths.contract, contract => {
    contract.task_manifests["axios-request-dispatch-v2"].sha256 = createHash("sha256")
      .update(readFileSync(paths.task))
      .digest("hex");
  });
}

function validateAxiosV2Fixture(dir, paths, contractRelative) {
  const contractBytes = readFileSync(paths.contract);
  const contract = JSON.parse(contractBytes);
  const identity = {
    corpus_id: contract.corpus_id,
    cache_id: "cold-inprocess-v1",
    machine_fingerprint: "fixture/fingerprint",
  };
  const corpusContract = {
    path: contractRelative,
    sha256: createHash("sha256").update(contractBytes).digest("hex"),
    corpus_id: contract.corpus_id,
    task_ids: contract.task_ids,
    runtime_modes: contract.runtime_modes,
    repeats: contract.repeats,
    task_manifests: contract.task_manifests,
    task_repositories: { "axios-request-dispatch-v2": "axios" },
    project_manifests: contract.project_manifests,
  };
  const rows = Array.from({ length: contract.repeats }, (_, index) => ({
    repo: "axios",
    task_id: "axios-request-dispatch-v2",
    mode: "cold_cli_packet",
    repeat: index + 1,
  }));
  return () => validatePacketCorpusContract(
    { corpus_contract: corpusContract },
    rows,
    dir,
    identity,
    { corpus_contract: { path: contractRelative, sha256: corpusContract.sha256 } },
  );
}

test("Axios v2 project manifest binding fails closed", async (t) => {
  await t.test("accepts the exact checked-in task, project, and corpus bytes", () => {
    const fixture = axiosV2CorpusFixture();
    assert.doesNotThrow(validateAxiosV2Fixture(fixture.dir, fixture.paths, fixture.contractRelative));
  });

  const failures = [
    ["missing", paths => rmSync(paths.project), /does not name a file/u],
    ["changed", paths => writeFileSync(paths.project, `${readFileSync(paths.project, "utf8")}\n`), /project manifest hash does not match/u],
    ["escaped", paths => {
      rewriteJson(paths.task, task => {
        task.repo.codestory_project_manifest.path = "../../../../outside-project.json";
      });
      rebindTaskHash(paths);
    }, /project manifest path .* escapes its allowed directory/u],
    ["substituted", paths => {
      const substitute = path.join(path.dirname(paths.project), "substitute-project.json");
      copyFileSync(paths.project, substitute);
      rewriteJson(paths.contract, contract => {
        contract.project_manifests["axios-request-dispatch-v2"].path
          = "benchmarks/tasks/release-evidence/substitute-project.json";
      });
    }, /path does not match task declaration/u],
    ["extra", paths => {
      rewriteJson(paths.contract, contract => {
        contract.project_manifests["extra-task"] = contract.project_manifests["axios-request-dispatch-v2"];
      });
    }, /keys must exactly match task declarations/u],
    ["task-inconsistent", paths => {
      rewriteJson(paths.task, task => {
        task.repo.codestory_project_manifest.sha256 = "0".repeat(64);
      });
      rebindTaskHash(paths);
    }, /hash does not match task declaration/u],
  ];
  for (const [name, mutate, expected] of failures) {
    await t.test(name, () => {
      const fixture = axiosV2CorpusFixture(mutate);
      assert.throws(validateAxiosV2Fixture(fixture.dir, fixture.paths, fixture.contractRelative), expected);
    });
  }
});

test("fingerprint prefers a validated provisioned machine identity", () => {
  const dir = mkdtempSync(path.join(tmpdir(), "codestory-provisioning-"));
  const profile = "codestory-release-evidence-linux-arm64-v1";
  const contractSha = "1".repeat(64);
  const observed = { guest: { arch: "aarch64" }, host: { model: "Mac17,4" } };
  const observedSha = createHash("sha256").update(`${JSON.stringify(observed)}\n`).digest("hex");
  const provisioning = path.join(dir, "provisioning.json");
  writeFileSync(provisioning, JSON.stringify({
    schema_version: 2,
    profile_id: profile,
    contract_sha256: contractSha,
    fingerprint: `${profile}/${contractSha}`,
    observed_identity: observed,
    observed_identity_sha256: observedSha,
  }));
  let result = spawnSync(process.execPath, [script, "fingerprint"], {
    encoding: "utf8",
    env: { ...process.env, CODESTORY_RELEASE_EVIDENCE_PROVISIONING: provisioning },
  });
  assert.equal(result.status, 0, result.stderr);
  assert.equal(result.stdout.trim(), `${profile}/${contractSha}`);

  const changed = JSON.parse(readFileSync(provisioning));
  changed.observed_identity.guest.arch = "x86_64";
  writeFileSync(provisioning, JSON.stringify(changed));
  result = spawnSync(process.execPath, [script, "fingerprint"], {
    encoding: "utf8",
    env: { ...process.env, CODESTORY_RELEASE_EVIDENCE_PROVISIONING: provisioning },
  });
  assert.notEqual(result.status, 0);
  assert.match(result.stderr, /observed identity attestation changed/);
});

test("prior-run reuse binds selected evidence to a failed trusted producer", () => {
  const dir = mkdtempSync(path.join(tmpdir(), "codestory-source-run-"));
  const runPath = path.join(dir, "run.json");
  const artifactsPath = path.join(dir, "artifacts.json");
  const run = {
    id: 42,
    path: ".github/workflows/packaged-platform-pr.yml",
    event: "workflow_dispatch",
    conclusion: "failure",
    head_sha: candidateSha,
    repository: { full_name: "TheGreenCedar/CodeStory" },
    head_repository: { full_name: "TheGreenCedar/CodeStory" },
  };
  const artifacts = {
    artifacts: [{
      name: `release-evidence-${candidateSha}`,
      expired: false,
      size_in_bytes: 1024,
      workflow_run: { id: run.id, head_sha: run.head_sha },
    }],
  };
  const validate = () => spawnSync(process.execPath, [
    script,
    "validate-source-run",
    "--run", runPath,
    "--artifacts", artifactsPath,
    "--repo", "TheGreenCedar/CodeStory",
    "--expected-sha", candidateSha,
  ], { encoding: "utf8" });

  writeFileSync(runPath, JSON.stringify(run));
  writeFileSync(artifactsPath, JSON.stringify(artifacts));
  assert.equal(validate().status, 0);

  run.path = ".github/workflows/arbitrary-pr.yml";
  writeFileSync(runPath, JSON.stringify(run));
  assert.match(validate().stderr, /not trusted evidence producers/);

  run.path = ".github/workflows/packaged-platform-pr.yml";
  run.head_sha = "3".repeat(40);
  artifacts.artifacts[0].workflow_run.head_sha = run.head_sha;
  writeFileSync(runPath, JSON.stringify(run));
  writeFileSync(artifactsPath, JSON.stringify(artifacts));
  assert.equal(validate().status, 0);

  run.path = ".github/workflows/release.yml";
  writeFileSync(runPath, JSON.stringify(run));
  assert.match(validate().stderr, /does not match the evidence SHA/);

  run.path = ".github/workflows/packaged-platform-pr.yml";
  artifacts.artifacts[0].name = `release-evidence-${"4".repeat(40)}`;
  writeFileSync(runPath, JSON.stringify(run));
  writeFileSync(artifactsPath, JSON.stringify(artifacts));
  assert.match(validate().stderr, /must contain exactly one release-evidence/);
});

test("checked-in candidate and report are deterministic and fully attested", () => {
  const dir = workspace();
  const out = path.join(dir, "report.json");
  const result = run("evaluate", ["--candidate", path.join(fixture, "candidate.json"), "--out", out]);
  assert.equal(result.status, 0, result.stderr);
  assert.deepEqual(JSON.parse(readFileSync(out)), JSON.parse(readFileSync(path.join(fixture, "report.json"))));
  const report = JSON.parse(readFileSync(out));
  assert.equal(report.decision, "accept_contract");
  assert.ok(report.artifact_paths.every(({ sha256, bytes }) => /^[0-9a-f]{64}$/.test(sha256) && bytes > 0));
});

test("packet selectors normalize to production transport row modes", () => {
  const dir = workspace();
  const packet = JSON.parse(readFileSync(path.join(dir, "packet.json")));
  assert.deepEqual(packet.modes, ["cold-cli"]);
  assert.deepEqual([...new Set(packet.release_evidence.rows.map((row) => row.mode))], ["cold_cli_packet"]);
  produce(dir);

  for (const row of packet.release_evidence.rows) row.mode = "warm_stdio_packet";
  writeFileSync(path.join(dir, "packet.json"), JSON.stringify(packet));
  const result = run("produce", ["--profile", "ci-contract-v1", "--stats", path.join(dir, "stats.json"), "--packet", path.join(dir, "packet.json"), "--out", path.join(dir, "candidate-2.json")]);
  assert.notEqual(result.status, 0);
  assert.match(result.stderr, /row modes do not match top-level modes/);
});

test("missing and all-zero raw artifacts fail production", () => {
  const dir = workspace();
  let result = run("produce", ["--profile", "ci-contract-v1", "--stats", path.join(dir, "missing.json"), "--packet", path.join(dir, "packet.json"), "--out", path.join(dir, "candidate.json")]);
  assert.notEqual(result.status, 0);
  const stats = JSON.parse(readFileSync(path.join(dir, "stats.json")));
  for (const key of ["retrieval_status_seconds", "ground_seconds", "repeat_full_refresh_seconds", "search_seconds", "index_seconds", "storage_bytes"]) stats[key] = 0;
  writeFileSync(path.join(dir, "stats.json"), JSON.stringify(stats));
  result = run("produce", ["--profile", "ci-contract-v1", "--stats", path.join(dir, "stats.json"), "--packet", path.join(dir, "packet.json"), "--out", path.join(dir, "candidate.json")]);
  assert.notEqual(result.status, 0);
  assert.match(result.stderr, /finite positive number/);
});

test("artifact mutation, identity drift, and short SHA are rejected", () => {
  const dir = workspace();
  produce(dir);
  writeFileSync(path.join(dir, "packet.json"), JSON.stringify({ summary: [{ median_e2e_wall_ms: 1 }] }));
  let result = run("evaluate", ["--candidate", path.join(dir, "candidate.json"), "--out", path.join(dir, "report.json")]);
  assert.notEqual(result.status, 0);
  assert.match(result.stderr, /artifact attestation changed/);
  result = spawnSync(process.execPath, [script, "evaluate", "--baseline", baseline, "--candidate", path.join(fixture, "candidate.json"), "--out", path.join(dir, "report.json"), "--expected-sha", candidateSha.slice(0, 8), "--release-key", "ci-contract-v1", "--mode", "contract", "--repo", root], { encoding: "utf8" });
  assert.notEqual(result.status, 0);
  assert.match(result.stderr, /full 40-character Git SHA/);
  const stats = JSON.parse(readFileSync(path.join(fixture, "candidate-stats.json")));
  stats.evidence_identity.cache_id = "different-cache";
  writeFileSync(path.join(dir, "stats.json"), JSON.stringify(stats));
  result = run("produce", ["--profile", "ci-contract-v1", "--stats", path.join(dir, "stats.json"), "--packet", path.join(fixture, "candidate-packet.json"), "--out", path.join(dir, "candidate-2.json")]);
  assert.notEqual(result.status, 0);
  assert.match(result.stderr, /cache_id does not match/);

  stats.commit = "1111111111111111111111111111111111111111";
  stats.evidence_identity.cache_id = "cold-full-retrieval-v1";
  writeFileSync(path.join(dir, "stats.json"), JSON.stringify(stats));
  result = spawnSync(process.execPath, [script, "produce", "--baseline", baseline, "--profile", "ci-contract-v1", "--stats", path.join(dir, "stats.json"), "--packet", path.join(fixture, "candidate-packet.json"), "--out", path.join(dir, "candidate-3.json"), "--expected-sha", stats.commit, "--release-key", "ci-contract-v1", "--mode", "contract", "--repo", root], { encoding: "utf8" });
  assert.notEqual(result.status, 0);
  assert.match(result.stderr, /identical/);
});

test("current full-product regressions are non-waivable and release-bound", () => {
  const dir = workspace();
  const stats = JSON.parse(readFileSync(path.join(dir, "stats.json")));
  stats.retrieval_status_seconds = 2;
  writeFileSync(path.join(dir, "stats.json"), JSON.stringify(stats));
  produce(dir);
  const reportPath = path.join(dir, "report.json");
  let result = run("evaluate", ["--candidate", path.join(dir, "candidate.json"), "--out", reportPath]);
  assert.equal(result.status, 1, result.stderr);
  const report = JSON.parse(readFileSync(reportPath));
  assert.equal(report.release_claim_evaluation.status, "fail");
  assert.ok(report.release_claim_evaluation.failures.some(({ class: failureClass, claim }) => failureClass === "failed_evidence" && claim === "performance"));
  const row = report.metrics.find(({ metric }) => metric === "status_seconds");
  const candidate = JSON.parse(readFileSync(path.join(dir, "candidate.json")));
  const answerQuality = candidate.release_claims.evidence.find(({ type }) => type === "answer_quality");
  const approvedAt = new Date().toISOString().slice(0, 10);
  const expiresAt = new Date(`${approvedAt}T00:00:00.000Z`);
  expiresAt.setUTCDate(expiresAt.getUTCDate() + 14);
  const approval = {
    schema_version: 3,
    metrics: {
      status_seconds: {
        candidate_sha256: report.candidate_sha256,
        commit: report.commit,
        profile: report.profile,
        baseline_id: report.baseline_id,
        baseline_sha256: report.baseline_sha256,
        metric: row.metric,
        regression_class: "model_microbenchmark",
        baseline_value: row.reference,
        measured_value: row.measured_value,
        threshold: row.threshold,
        regression_percent: 100,
        direction: "max",
        repeats: 3,
        release_key: candidate.release_key,
        owner: "release owner",
        approved_at: approvedAt,
        expires_at: expiresAt.toISOString().slice(0, 10),
        rationale: "Attempted full-product exception",
        rollback_evidence: "Revert the candidate and restore the accepted baseline",
        full_product_benefit: {
          evidence_id: answerQuality.id,
          artifact_sha256: candidate.artifacts.packet.sha256,
          observed_at: answerQuality.observed_at,
          metric: "packet_quality_score",
          baseline_value: 0.5,
          measured_value: 0.6,
          direction: "increase",
          improvement_percent: 20
        }
      }
    }
  };
  const approvalPath = path.join(dir, "approval.json");
  writeFileSync(approvalPath, JSON.stringify(approval));
  result = run("evaluate", ["--candidate", path.join(dir, "candidate.json"), "--approval", approvalPath, "--out", reportPath]);
  assert.notEqual(result.status, 0);
  assert.match(result.stderr, /status_seconds is a non-waivable full-product gate/u);

  approval.schema_version = 2;
  writeFileSync(approvalPath, JSON.stringify(approval));
  result = run("evaluate", ["--candidate", path.join(dir, "candidate.json"), "--approval", approvalPath, "--out", reportPath]);
  assert.notEqual(result.status, 0);
  assert.match(result.stderr, /approval schema_version must be 3/u);

  result = run("evaluate", [
    "--candidate", path.join(dir, "candidate.json"),
    "--out", reportPath,
    "--release-key", "next-release",
  ]);
  assert.notEqual(result.status, 0);
  assert.match(result.stderr, /candidate release_key does not match/u);
});

test("evaluation independently rejects reattested raw commit and packet provenance drift", () => {
  const dir = workspace();
  produce(dir);
  let candidate = JSON.parse(readFileSync(path.join(dir, "candidate.json")));
  const statsPath = path.join(dir, "stats.json");
  const stats = JSON.parse(readFileSync(statsPath));
  stats.commit = "3333333333333333333333333333333333333333";
  writeFileSync(statsPath, JSON.stringify(stats));
  reattest(candidate, "stats", statsPath);
  writeFileSync(path.join(dir, "candidate.json"), JSON.stringify(candidate));
  let result = run("evaluate", ["--candidate", path.join(dir, "candidate.json"), "--out", path.join(dir, "report.json")]);
  assert.notEqual(result.status, 0);
  assert.match(result.stderr, /raw stats commit/);

  const dir2 = workspace();
  produce(dir2);
  candidate = JSON.parse(readFileSync(path.join(dir2, "candidate.json")));
  const packetPath = path.join(dir2, "packet.json");
  const packet = JSON.parse(readFileSync(packetPath));
  packet.release_evidence.publishable = false;
  packet.release_evidence.repeats = 1;
  packet.release_evidence.quality_gate_status = "fail";
  writeFileSync(packetPath, JSON.stringify(packet));
  reattest(candidate, "packet", packetPath);
  writeFileSync(path.join(dir2, "candidate.json"), JSON.stringify(candidate));
  result = run("evaluate", ["--candidate", path.join(dir2, "candidate.json"), "--out", path.join(dir2, "report.json")]);
  assert.notEqual(result.status, 0);
  assert.match(result.stderr, /not publishable three-repeat quality-gated/);
});

test("failed packet quality and non-full stats cannot produce or evaluate a passing candidate", () => {
  const dir = workspace();
  const packetPath = path.join(dir, "packet.json");
  const packet = JSON.parse(readFileSync(packetPath));
  packet.release_evidence.rows[0].quality.pass = false;
  writeFileSync(packetPath, JSON.stringify(packet));
  let result = run("produce", ["--profile", "ci-contract-v1", "--stats", path.join(dir, "stats.json"), "--packet", packetPath, "--out", path.join(dir, "candidate.json")]);
  assert.notEqual(result.status, 0);
  assert.match(result.stderr, /quality row 0/);

  const dir2 = workspace();
  produce(dir2);
  const statsPath = path.join(dir2, "stats.json");
  const stats = JSON.parse(readFileSync(statsPath));
  stats.proof_tier = "stats_only";
  writeFileSync(statsPath, JSON.stringify(stats));
  const candidate = JSON.parse(readFileSync(path.join(dir2, "candidate.json")));
  reattest(candidate, "stats", statsPath);
  writeFileSync(path.join(dir2, "candidate.json"), JSON.stringify(candidate));
  result = run("evaluate", ["--candidate", path.join(dir2, "candidate.json"), "--out", path.join(dir2, "report.json")]);
  assert.notEqual(result.status, 0);
  assert.match(result.stderr, /full-retrieval readiness contract/);
});

test("release evidence independently enforces the versioned claim graph", () => {
  const dir = workspace();
  produce(dir);
  const candidatePath = path.join(dir, "candidate.json");
  const reportPath = path.join(dir, "report.json");
  let candidate = JSON.parse(readFileSync(candidatePath));
  candidate.release_claims.requested_claims = candidate.release_claims.requested_claims
    .filter(({ id }) => id === "retrieval_readiness");
  candidate.release_claims.evidence = candidate.release_claims.evidence
    .filter(({ type }) => type === "retrieval_readiness");
  writeFileSync(candidatePath, JSON.stringify(candidate));
  let result = run("evaluate", ["--candidate", candidatePath, "--out", reportPath]);
  assert.notEqual(result.status, 0);
  assert.match(result.stderr, /must exactly match the trusted release-evidence claim profile/u);

  produce(dir);
  candidate = JSON.parse(readFileSync(candidatePath));
  candidate.release_claims.evidence.find(({ type }) => type === "answer_quality").tier = "live_behavior";
  writeFileSync(candidatePath, JSON.stringify(candidate));
  result = run("evaluate", ["--candidate", candidatePath, "--out", reportPath]);
  assert.notEqual(result.status, 0);
  assert.match(result.stderr, /incompatible_tier_identity/u);

  produce(dir);
  candidate = JSON.parse(readFileSync(candidatePath));
  candidate.release_claims.evidence[0].graph_sha256 = "0".repeat(64);
  writeFileSync(candidatePath, JSON.stringify(candidate));
  result = run("evaluate", ["--candidate", candidatePath, "--out", reportPath]);
  assert.notEqual(result.status, 0);
  assert.match(result.stderr, /stale_evidence/u);

  produce(dir);
  candidate = JSON.parse(readFileSync(candidatePath));
  candidate.release_claims.evidence.find(({ type }) => type === "performance")
    .identity.baseline_id = "fabricated@baseline";
  writeFileSync(candidatePath, JSON.stringify(candidate));
  result = run("evaluate", ["--candidate", candidatePath, "--out", reportPath]);
  assert.notEqual(result.status, 0);
  assert.match(result.stderr, /incompatible_tier_identity/u);

  produce(dir);
  candidate = JSON.parse(readFileSync(candidatePath));
  candidate.release_claims.evidence.find(({ type }) => type === "answer_quality")
    .identity.evaluation_contract = "fabricated/v9";
  writeFileSync(candidatePath, JSON.stringify(candidate));
  result = run("evaluate", ["--candidate", candidatePath, "--out", reportPath]);
  assert.notEqual(result.status, 0);
  assert.match(result.stderr, /incompatible_tier_identity/u);
});

test("forged provenance, duplicate repeats, and omitted repeat budgets fail", () => {
  const dir = workspace();
  let packet = JSON.parse(readFileSync(path.join(dir, "packet.json")));
  packet.release_evidence.rows[0].repo_provenance = {};
  packet.release_evidence.rows[0].codestory_cache_provenance = {};
  writeFileSync(path.join(dir, "packet.json"), JSON.stringify(packet));
  let result = run("produce", ["--profile", "ci-contract-v1", "--stats", path.join(dir, "stats.json"), "--packet", path.join(dir, "packet.json"), "--out", path.join(dir, "candidate.json")]);
  assert.notEqual(result.status, 0);
  assert.match(result.stderr, /provenance row 0 failed/);

  const dir2 = workspace();
  packet = JSON.parse(readFileSync(path.join(dir2, "packet.json")));
  packet.release_evidence.rows[1].repeat = 1;
  writeFileSync(path.join(dir2, "packet.json"), JSON.stringify(packet));
  result = run("produce", ["--profile", "ci-contract-v1", "--stats", path.join(dir2, "stats.json"), "--packet", path.join(dir2, "packet.json"), "--out", path.join(dir2, "candidate.json")]);
  assert.notEqual(result.status, 0);
  assert.match(result.stderr, /duplicate repeat 1/);

  const dir3 = workspace();
  produce(dir3);
  const statsPath = path.join(dir3, "stats.json");
  const stats = JSON.parse(readFileSync(statsPath));
  delete stats.repeat_semantic_phase_seconds;
  writeFileSync(statsPath, JSON.stringify(stats));
  const candidate = JSON.parse(readFileSync(path.join(dir3, "candidate.json")));
  reattest(candidate, "stats", statsPath);
  writeFileSync(path.join(dir3, "candidate.json"), JSON.stringify(candidate));
  result = run("evaluate", ["--candidate", path.join(dir3, "candidate.json"), "--out", path.join(dir3, "report.json")]);
  assert.notEqual(result.status, 0);
  assert.match(result.stderr, /repeat-refresh release contract/);
});
