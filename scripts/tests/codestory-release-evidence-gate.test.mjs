import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { copyFileSync, mkdtempSync, readFileSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import { spawnSync } from "node:child_process";
import test from "node:test";
import { fileURLToPath } from "node:url";

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "../..");
const script = path.join(root, "scripts/codestory-release-evidence-gate.mjs");
const fixture = path.join(root, "benchmarks/release-evidence/fixtures");
const baseline = path.join(root, "benchmarks/release-evidence/approved-baselines.json");
const candidateSha = "2222222222222222222222222222222222222222";

function run(command, extra = []) {
  return spawnSync(process.execPath, [script, command, "--baseline", baseline, "--expected-sha", candidateSha, "--mode", "contract", "--repo", root, ...extra], { encoding: "utf8" });
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
  result = spawnSync(process.execPath, [script, "evaluate", "--baseline", baseline, "--candidate", path.join(fixture, "candidate.json"), "--out", path.join(dir, "report.json"), "--expected-sha", candidateSha.slice(0, 8), "--mode", "contract", "--repo", root], { encoding: "utf8" });
  assert.notEqual(result.status, 0);
  assert.match(result.stderr, /full 40-character Git SHA/);
  const stats = JSON.parse(readFileSync(path.join(fixture, "candidate-stats.json")));
  stats.evidence_identity.cache_id = "different-cache";
  writeFileSync(path.join(dir, "stats.json"), JSON.stringify(stats));
  result = run("produce", ["--profile", "ci-contract-v1", "--stats", path.join(dir, "stats.json"), "--packet", path.join(fixture, "candidate-packet.json"), "--out", path.join(dir, "candidate-2.json")]);
  assert.notEqual(result.status, 0);
  assert.match(result.stderr, /cache_id does not match/);

  stats.commit = "1111111111111111111111111111111111111111";
  stats.evidence_identity.cache_id = "cold-full-sidecar-v1";
  writeFileSync(path.join(dir, "stats.json"), JSON.stringify(stats));
  result = spawnSync(process.execPath, [script, "produce", "--baseline", baseline, "--profile", "ci-contract-v1", "--stats", path.join(dir, "stats.json"), "--packet", path.join(fixture, "candidate-packet.json"), "--out", path.join(dir, "candidate-3.json"), "--expected-sha", stats.commit, "--mode", "contract", "--repo", root], { encoding: "utf8" });
  assert.notEqual(result.status, 0);
  assert.match(result.stderr, /identical/);
});

test("regression approval is bound to candidate hash, value, threshold, baseline, profile, and expiry", () => {
  const dir = workspace();
  const stats = JSON.parse(readFileSync(path.join(dir, "stats.json")));
  stats.retrieval_status_seconds = 2;
  writeFileSync(path.join(dir, "stats.json"), JSON.stringify(stats));
  produce(dir);
  const reportPath = path.join(dir, "report.json");
  let result = run("evaluate", ["--candidate", path.join(dir, "candidate.json"), "--out", reportPath]);
  assert.equal(result.status, 1, result.stderr);
  const report = JSON.parse(readFileSync(reportPath));
  const row = report.metrics.find(({ metric }) => metric === "status_seconds");
  const approval = {
    schema_version: 2,
    metrics: {
      status_seconds: {
        candidate_sha256: report.candidate_sha256,
        commit: report.commit,
        profile: report.profile,
        baseline_id: report.baseline_id,
        baseline_sha256: report.baseline_sha256,
        metric: row.metric,
        measured_value: row.measured_value,
        threshold: row.threshold,
        owner: "release owner",
        approved_at: "2026-07-11",
        expires_at: "2099-07-11",
        rationale: "Bound contract exception"
      }
    }
  };
  const approvalPath = path.join(dir, "approval.json");
  writeFileSync(approvalPath, JSON.stringify(approval));
  result = run("evaluate", ["--candidate", path.join(dir, "candidate.json"), "--approval", approvalPath, "--out", reportPath]);
  assert.equal(result.status, 0, result.stderr);
  approval.metrics.status_seconds.expires_at = "2026-07-12";
  writeFileSync(approvalPath, JSON.stringify(approval));
  result = run("evaluate", ["--candidate", path.join(dir, "candidate.json"), "--approval", approvalPath, "--out", reportPath]);
  assert.notEqual(result.status, 0);
  assert.match(result.stderr, /approval is expired/);
  approval.metrics.status_seconds.expires_at = "2099-07-11";
  approval.metrics.status_seconds.measured_value = 1.9;
  writeFileSync(approvalPath, JSON.stringify(approval));
  result = run("evaluate", ["--candidate", path.join(dir, "candidate.json"), "--approval", approvalPath, "--out", reportPath]);
  assert.notEqual(result.status, 0);
  assert.match(result.stderr, /does not match measured evidence/);
  approval.metrics.status_seconds.measured_value = row.measured_value;
  approval.metrics.status_seconds.approved_at = "2026-02-31";
  writeFileSync(approvalPath, JSON.stringify(approval));
  result = run("evaluate", ["--candidate", path.join(dir, "candidate.json"), "--approval", approvalPath, "--out", reportPath]);
  assert.notEqual(result.status, 0);
  assert.match(result.stderr, /valid ISO date/);
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
  assert.match(result.stderr, /full-sidecar readiness contract/);
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
